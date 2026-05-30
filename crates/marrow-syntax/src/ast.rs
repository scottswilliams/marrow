//! Abstract syntax tree: the data types produced by parsing a Marrow source
//! file, together with their small accessor impls.

use std::fmt;

use crate::{Diagnostic, Severity, SourceSpan};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSource {
    pub file: SourceFile,
    pub diagnostics: Vec<Diagnostic>,
}

impl ParsedSource {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceFile {
    pub module: Option<ModuleDecl>,
    pub uses: Vec<UseDecl>,
    pub declarations: Vec<Declaration>,
}

impl SourceFile {
    pub fn resource(&self, name: &str) -> Option<&ResourceDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Resource(resource) if resource.name == name => Some(resource),
                _ => None,
            })
    }

    pub fn function(&self, name: &str) -> Option<&FunctionDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == name => Some(function),
                _ => None,
            })
    }

    pub fn enum_decl(&self, name: &str) -> Option<&EnumDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Enum(decl) if decl.name == name => Some(decl),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDecl {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseDecl {
    pub name: String,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Declaration {
    Const(ConstDecl),
    Resource(ResourceDecl),
    Function(FunctionDecl),
    Enum(EnumDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub ty: Option<TypeRef>,
    /// `None` when the value text did not parse as an expression; the parser
    /// reports a syntax error in that case.
    pub value: Option<Expression>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expression {
    Literal {
        kind: LiteralKind,
        text: String,
        span: SourceSpan,
    },
    /// A name path of one or more `::`-separated identifiers, such as `x` or
    /// `std::math::PI`.
    Name {
        segments: Vec<String>,
        span: SourceSpan,
    },
    /// A saved-data root such as `^books`. Postfix key lookups and field
    /// access build the rest of a saved path on top of this.
    SavedRoot { name: String, span: SourceSpan },
    /// A parenthesized application: a function call, key lookup, conversion, or
    /// resource constructor. The checker resolves which one from the callee.
    Call {
        callee: Box<Expression>,
        args: Vec<Argument>,
        span: SourceSpan,
    },
    /// Dotted field access, such as `book.title` or `^books(id)."old-title"`.
    /// `name` is the field name without surrounding quotes; `quoted` records
    /// whether it was written as a quoted segment (allowed for data names that
    /// are not identifiers).
    Field {
        base: Box<Expression>,
        name: String,
        quoted: bool,
        span: SourceSpan,
    },
    /// Optional field access `base?.name`: the same read as `Field`, but an
    /// absent base or field short-circuits the rest of the chain to absent
    /// rather than failing the read. The leaf type matches the plain field.
    OptionalField {
        base: Box<Expression>,
        name: String,
        quoted: bool,
        span: SourceSpan,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Expression>,
        span: SourceSpan,
    },
    Binary {
        op: BinaryOp,
        left: Box<Expression>,
        right: Box<Expression>,
        span: SourceSpan,
    },
    /// An interpolated string `$"..."` as a sequence of literal text and
    /// embedded expression parts, in source order.
    Interpolation {
        parts: Vec<InterpolationPart>,
        span: SourceSpan,
    },
}

impl Expression {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Literal { span, .. }
            | Self::Name { span, .. }
            | Self::SavedRoot { span, .. }
            | Self::Call { span, .. }
            | Self::Field { span, .. }
            | Self::OptionalField { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Interpolation { span, .. } => *span,
        }
    }
}

/// One segment of an interpolated string: either literal text (with `{{`/`}}`
/// still escaped as written) or an embedded expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpolationPart {
    Text { text: String, span: SourceSpan },
    Expr(Expression),
}

/// One argument in a call expression. `name` is set for named arguments
/// (`title: draft`); `mode` is set for `out`/`inout` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub mode: Option<ArgMode>,
    pub name: Option<String>,
    pub value: Expression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgMode {
    Out,
    InOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Integer,
    Decimal,
    String,
    Bytes,
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Multiply,
    Divide,
    Remainder,
    Add,
    Subtract,
    Concat,
    RangeExclusive,
    RangeInclusive,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    /// The absence-default `??`: yields the left path read when populated, else
    /// the right default. The left operand must be a path read or `?.` chain.
    Coalesce,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub store: Option<SavedRoot>,
    pub members: Vec<ResourceMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRoot {
    pub root: String,
    pub keys: Vec<KeyParam>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceMember {
    Field(FieldDecl),
    Group(GroupDecl),
    Index(IndexDecl),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub required: bool,
    pub name: String,
    pub keys: Vec<KeyParam>,
    pub ty: TypeRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub keys: Vec<KeyParam>,
    pub members: Vec<ResourceMember>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDecl {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub args: Vec<String>,
    pub unique: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub return_type: Option<TypeRef>,
    pub body: Block,
    pub span: SourceSpan,
}

/// A flat enum: a named, fixed set of bare member values, generalizing `bool`.
/// `public` is recorded for `pub enum` consistency with `pub fn`; it is not yet
/// enforced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub members: Vec<EnumMember>,
    pub span: SourceSpan,
}

/// One enum member: a bare identifier. `stable_id` is a reserved slot for the
/// rename-safe stable-id work; the parser always leaves it `None` for now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMember {
    pub docs: Vec<String>,
    pub stable_id: Option<String>,
    pub name: String,
    pub span: SourceSpan,
}

/// An indented sequence of statements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    pub statements: Vec<Statement>,
    /// Ordinary `;` comments inside this block, in source order. They are kept
    /// as block-level trivia (not attached to statement nodes) so the formatter
    /// can re-emit them and `parse -> format` round-trips comments losslessly.
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// An ordinary `;` comment retained as block trivia. `text` is the comment body
/// with the leading `;` marker and surrounding whitespace removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub placement: CommentPlacement,
    pub span: SourceSpan,
}

/// Where a retained comment sits relative to the statements of its block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentPlacement {
    /// A comment occupying its own line (a leading or standalone comment).
    OwnLine,
    /// A comment following code on a statement's line.
    Trailing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Statement {
    Const {
        name: String,
        ty: Option<TypeRef>,
        value: Expression,
        span: SourceSpan,
    },
    Var {
        name: String,
        keys: Vec<KeyParam>,
        ty: Option<TypeRef>,
        value: Option<Expression>,
        span: SourceSpan,
    },
    Assign {
        target: Expression,
        value: Expression,
        span: SourceSpan,
    },
    Delete {
        path: Expression,
        span: SourceSpan,
    },
    Merge {
        target: Expression,
        value: Expression,
        span: SourceSpan,
    },
    Return {
        value: Option<Expression>,
        span: SourceSpan,
    },
    Break {
        label: Option<String>,
        span: SourceSpan,
    },
    Continue {
        label: Option<String>,
        span: SourceSpan,
    },
    Throw {
        value: Expression,
        span: SourceSpan,
    },
    Expr {
        value: Expression,
        span: SourceSpan,
    },
    If {
        condition: Option<Expression>,
        then_block: Block,
        else_ifs: Vec<ElseIf>,
        else_block: Option<Block>,
        span: SourceSpan,
    },
    While {
        label: Option<String>,
        condition: Option<Expression>,
        body: Block,
        span: SourceSpan,
    },
    For {
        label: Option<String>,
        binding: ForBinding,
        iterable: Expression,
        body: Block,
        span: SourceSpan,
    },
    Transaction {
        body: Block,
        span: SourceSpan,
    },
    Lock {
        path: Option<Expression>,
        body: Block,
        span: SourceSpan,
    },
    Try {
        body: Block,
        catch: Option<CatchClause>,
        finally: Option<Block>,
        span: SourceSpan,
    },
    /// A `match` over an enum-typed scrutinee: each arm names one member of the
    /// enum and holds the block to run when the scrutinee selects it. Arms name a
    /// bare member (the scrutinee supplies the enum); a local enum's `match` has
    /// no wildcard arm. Exhaustiveness and member validity are checker rules.
    ///
    /// `enum_name`/`enum_module` are the scrutinee's resolved enum identity,
    /// filled by the checker (`enum_module` is the owning module's qualified name,
    /// empty for a module-less script). The parser leaves both `None`; the runtime
    /// dispatches arms by that exact enum's ordinals, so two enums that share
    /// member names — even across modules with the same enum name — never alias.
    Match {
        scrutinee: Option<Expression>,
        arms: Vec<MatchArm>,
        enum_name: Option<String>,
        enum_module: Option<String>,
        span: SourceSpan,
    },
}

/// One arm of a `match` statement: a bare member name and the block run when the
/// scrutinee selects that member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub member: String,
    pub block: Block,
    pub span: SourceSpan,
}

/// One `else if` clause of an `if` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIf {
    /// `None` when the condition text did not parse as an expression.
    pub condition: Option<Expression>,
    pub block: Block,
}

/// The `catch name: Error` clause of a `try` statement. `ty` is the optional
/// type annotation on the bound error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchClause {
    pub name: String,
    pub ty: Option<TypeRef>,
    pub block: Block,
}

/// The loop variable(s) of a `for` statement: `for first in ...` or
/// `for first, second in ...`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForBinding {
    pub first: String,
    pub second: Option<String>,
}

impl Statement {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Const { span, .. }
            | Self::Var { span, .. }
            | Self::Assign { span, .. }
            | Self::Delete { span, .. }
            | Self::Merge { span, .. }
            | Self::Return { span, .. }
            | Self::Break { span, .. }
            | Self::Continue { span, .. }
            | Self::Throw { span, .. }
            | Self::Expr { span, .. }
            | Self::If { span, .. }
            | Self::While { span, .. }
            | Self::For { span, .. }
            | Self::Transaction { span, .. }
            | Self::Lock { span, .. }
            | Self::Try { span, .. }
            | Self::Match { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    /// The run of `;;` doc lines written directly above this parameter, one
    /// entry per line in source order. Empty for a single-line list, where
    /// parameter docs are not written.
    pub docs: Vec<String>,
    pub mode: Option<ParamMode>,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Out,
    InOut,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParam {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub text: String,
}

impl fmt::Display for TypeRef {
    // The parser keeps the verbatim source spelling so the formatter re-emits a
    // type annotation exactly as written. Resolution to a structured type happens
    // once in marrow-schema; this text is the AST's only remaining use of it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}
