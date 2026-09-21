//! The declaration and statement parsers: a recursive-descent parser over the
//! file-wide token stream that frames resource, enum, and function bodies, and
//! the statement parser it delegates to. Together with the free token-walking
//! helpers they structure everything above the expression level.
//!
//! The module is split by concern: `decl` drives top-level dispatch and the
//! declaration bodies, `cursor` is the shared `DeclParser` navigation and error
//! surface, `members` carries the resource/enum bodies, `head` and `params`
//! parse the token-level declaration heads, `stmt` is the
//! statement parser, `statement_lines` parses single statement lines, and
//! `tokens` holds the low-level token-slice helpers shared across all of them.

mod body;
mod cursor;
mod decl;
mod head;
mod members;
mod params;
mod statement_lines;
mod stmt;
mod tokens;

pub(crate) use decl::DeclParser;

use crate::ast::{KeyParam, ParamDecl, TypeExpr, TypeParamDecl};
use crate::diagnostic::{ParseDiagnosticReason, SourceSpan};

pub(super) struct FunctionHead {
    public: bool,
    name: String,
    name_span: SourceSpan,
    type_params: Vec<TypeParamDecl>,
    params: Vec<ParamDecl>,
    return_type: Option<TypeExpr>,
}

pub(super) enum MemberHead {
    Field {
        required: bool,
        name: String,
        name_span: SourceSpan,
        keys: Vec<KeyParam>,
        ty: TypeExpr,
    },
    Group {
        name: String,
        name_span: SourceSpan,
        keys: Vec<KeyParam>,
    },
}

#[derive(Debug, Clone)]
pub(super) struct ParseError {
    reason: ParseDiagnosticReason,
    message: String,
    /// The span to report at, when the error already knows the offending token.
    /// Errors without one are reported at a fallback span the caller supplies.
    span: Option<SourceSpan>,
}

impl ParseError {
    pub(super) fn new(reason: ParseDiagnosticReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
            span: None,
        }
    }

    pub(super) fn at(
        span: SourceSpan,
        reason: ParseDiagnosticReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            reason,
            message: message.into(),
            span: Some(span),
        }
    }

    /// Resolve the error into its report span, reason, and message, falling back
    /// to `fallback` when the error did not pin a span.
    pub(super) fn locate(
        self,
        fallback: SourceSpan,
    ) -> (SourceSpan, ParseDiagnosticReason, String) {
        (self.span.unwrap_or(fallback), self.reason, self.message)
    }
}

pub(super) type ParseResult<T> = Result<T, ParseError>;
