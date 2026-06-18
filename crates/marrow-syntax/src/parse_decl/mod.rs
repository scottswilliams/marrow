//! The declaration and statement parsers: a recursive-descent parser over the
//! file-wide token stream that frames resource, enum, and function bodies, and
//! the statement parser it delegates to. Together with the free token-walking
//! helpers they structure everything above the expression level.
//!
//! The module is split by concern: `decl` drives top-level dispatch and the
//! declaration bodies, `cursor` is the shared `DeclParser` navigation and error
//! surface, `members` and `evolve` carry the resource/enum and evolution bodies,
//! `head` and `params` parse the token-level declaration heads, `stmt` is the
//! statement parser, `statement_lines` parses single statement lines, and
//! `tokens` holds the low-level token-slice helpers shared across all of them.

mod cursor;
mod decl;
mod evolve;
mod head;
mod members;
mod params;
mod statement_lines;
mod stmt;
mod surface;
mod tokens;

pub(crate) use decl::DeclParser;

use crate::ast::{FunctionReturnPresence, KeyParam, ParamDecl, TypeRef};
use crate::diagnostic::ParseDiagnosticReason;

pub(super) struct FunctionHead {
    public: bool,
    name: String,
    params: Vec<ParamDecl>,
    return_presence: FunctionReturnPresence,
    return_type: Option<TypeRef>,
}

pub(super) enum MemberHead {
    Field {
        required: bool,
        name: String,
        keys: Vec<KeyParam>,
        ty: TypeRef,
    },
    Group {
        name: String,
        keys: Vec<KeyParam>,
    },
}

/// Classification of the next line in an indented member block, after the shared
/// trivia (dedent, blank lines, comments, stray indents) has been handled.
pub(super) enum MemberBlockFrame {
    /// The block closed on its `DEDENT`; stop the loop.
    Done,
    /// A trivia line was consumed; continue without parsing a member.
    Trivia,
    /// A member header is in place for the caller to parse.
    Member,
}

#[derive(Debug, Clone)]
pub(super) struct ParseError {
    reason: ParseDiagnosticReason,
    message: String,
}

impl ParseError {
    pub(super) fn new(reason: ParseDiagnosticReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

pub(super) type ParseResult<T> = Result<T, ParseError>;
