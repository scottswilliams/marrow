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
    pub comments: Vec<Comment>,
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

    pub fn store(&self, root: &str) -> Option<&StoreDecl> {
        self.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Store(store) if store.root.root == root => Some(store),
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
    Store(StoreDecl),
    Surface(SurfaceDecl),
    Function(FunctionDecl),
    Enum(EnumDecl),
    Evolve(EvolveDecl),
}

/// An `evolve` block: the source's explicit intent for catalog-addressable
/// entities. A bare source diff implies no intent, so identity-preserving and
/// destructive changes are stated here rather than inferred from edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvolveDecl {
    pub steps: Vec<EvolveStep>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// One evolution intent. Each step's target is a path expression naming a
/// catalog-addressable entity, written in the same surface forms the language
/// already uses for such references (`Book.title`, `^books`, `^books.byTitle`,
/// `Status::archived`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveStep {
    /// `from` and `to` name the same durable entity, so stable identity and
    /// stored data carry over the rename.
    Rename {
        from: Expression,
        to: Expression,
        span: SourceSpan,
    },
    /// `value` backfills existing records that lack `target`.
    Default {
        target: Expression,
        value: Expression,
        span: SourceSpan,
    },
    /// Destructive intent to remove the entity and its stored data.
    Retire {
        target: Expression,
        span: SourceSpan,
    },
    /// `transform <target> NEWLINE INDENT statement+ DEDENT`: a checked transform
    /// computing the new shape of `target` from the old.
    Transform {
        target: Expression,
        body: Block,
        span: SourceSpan,
    },
}

impl EvolveStep {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Rename { span, .. }
            | Self::Default { span, .. }
            | Self::Retire { span, .. }
            | Self::Transform { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub ty: Option<TypeRef>,
    /// `None` when the value text did not parse; the parser reports the error.
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
        segment_spans: Vec<SourceSpan>,
        span: SourceSpan,
    },
    /// A saved-data root such as `^books`. Postfix key lookups and field access
    /// build the rest of a saved path on top of this.
    SavedRoot { name: String, span: SourceSpan },
    /// A parenthesized application: the checker resolves call, key lookup,
    /// conversion, or constructor from the callee.
    Call {
        callee: Box<Expression>,
        args: Vec<Argument>,
        multiline: bool,
        span: SourceSpan,
    },
    /// `name` is the field name unquoted; `quoted` records whether it was
    /// written as a quoted segment (for data names that are not identifiers).
    Field {
        base: Box<Expression>,
        name: String,
        name_span: SourceSpan,
        quoted: bool,
        span: SourceSpan,
    },
    /// Like `Field`, but an absent base or field short-circuits the rest of the
    /// chain to absent rather than failing the read.
    OptionalField {
        base: Box<Expression>,
        name: String,
        name_span: SourceSpan,
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
    Range {
        start: Option<Box<Expression>>,
        end: Option<Box<Expression>>,
        inclusive_end: bool,
        step: Option<Box<Expression>>,
        span: SourceSpan,
    },
    /// An interpolated string `$"..."` as a sequence of literal text and embedded
    /// expression parts, in source order.
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
            | Self::Range { span, .. }
            | Self::Interpolation { span, .. } => *span,
        }
    }
}

pub struct RangeExpr<'a> {
    pub start: Option<&'a Expression>,
    pub end: Option<&'a Expression>,
    pub inclusive_end: bool,
    pub step: Option<&'a Expression>,
    pub span: SourceSpan,
}

pub fn range_expr(expr: &Expression) -> Option<RangeExpr<'_>> {
    match expr {
        Expression::Binary {
            op: BinaryOp::RangeExclusive | BinaryOp::RangeInclusive,
            left,
            right,
            span,
        } => Some(RangeExpr {
            start: Some(left),
            end: Some(right),
            inclusive_end: matches!(
                expr,
                Expression::Binary {
                    op: BinaryOp::RangeInclusive,
                    ..
                }
            ),
            step: None,
            span: *span,
        }),
        Expression::Range {
            start,
            end,
            inclusive_end,
            step,
            span,
        } => Some(RangeExpr {
            start: start.as_deref(),
            end: end.as_deref(),
            inclusive_end: *inclusive_end,
            step: step.as_deref(),
            span: *span,
        }),
        _ => None,
    }
}

/// `Text` keeps `{{`/`}}` escaped as written; decoding happens downstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpolationPart {
    Text { text: String, span: SourceSpan },
    Expr(Expression),
}

/// One argument in a call expression. `name` is set for named arguments
/// (`title: draft`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub name: Option<String>,
    pub value: Expression,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Integer,
    Decimal,
    /// A duration literal `NUMBER.UNIT` (`1.day`); the token text is the whole
    /// literal and [`crate::duration_unit_seconds`] validates the unit.
    Duration,
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
    /// The enum-subtree test `is`: true when the left value sits at or under the
    /// right member in its enum's hierarchy. Exact for a concrete leaf, a subtree
    /// test for a category. Complements `==`, which is exact nominal equality.
    Is,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    pub members: Vec<ResourceMember>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreDecl {
    pub docs: Vec<String>,
    pub root: SavedRoot,
    pub resource: String,
    pub indexes: Vec<IndexDecl>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceDecl {
    pub name: String,
    pub store: SavedRoot,
    pub items: Vec<SurfaceItem>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceItem {
    Fields {
        names: Vec<String>,
        span: SourceSpan,
    },
    Collection {
        target: SurfaceTarget,
        alias: String,
        span: SourceSpan,
    },
    Action {
        function: Vec<String>,
        alias: String,
        span: SourceSpan,
    },
    Create {
        names: Vec<String>,
        span: SourceSpan,
    },
    Update {
        names: Vec<String>,
        span: SourceSpan,
    },
    Delete {
        span: SourceSpan,
    },
}

impl SurfaceItem {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Fields { span, .. }
            | Self::Collection { span, .. }
            | Self::Action { span, .. }
            | Self::Create { span, .. }
            | Self::Update { span, .. }
            | Self::Delete { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceTarget {
    Root { root: String },
    Index { root: String, index: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRoot {
    pub root: String,
    pub keys: Vec<KeyParam>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceMember {
    Field(FieldDecl),
    Group(GroupDecl),
}

impl ResourceMember {
    pub fn span(&self) -> SourceSpan {
        match self {
            ResourceMember::Field(field) => field.span,
            ResourceMember::Group(group) => group.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDecl {
    pub docs: Vec<String>,
    pub required: bool,
    pub name: String,
    pub name_span: SourceSpan,
    pub keys: Vec<KeyParam>,
    pub ty: TypeRef,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    pub keys: Vec<KeyParam>,
    pub members: Vec<ResourceMember>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
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
    pub return_presence: FunctionReturnPresence,
    pub return_type: Option<TypeRef>,
    pub body: Block,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionReturnPresence {
    Always,
    MaybePresent,
}

/// An enum: a named, fixed set of member values, generalizing `bool`. Members may
/// nest into a tree (`Cat::tiger::bengal`); a flat enum is the degenerate
/// one-level tree. `public` records `pub enum` visibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub name_span: SourceSpan,
    pub members: Vec<EnumMember>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// One enum member: a bare identifier, optionally with nested members under it.
/// A `category` member groups its descendants and is not selectable as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMember {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    pub category: bool,
    pub members: Vec<EnumMember>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// An indented sequence of statements.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    pub statements: Vec<Statement>,
    /// Line comments inside this block, in source order. They are kept as
    /// block-level trivia (not attached to statement nodes) so the formatter can
    /// re-emit them and `parse -> format` round-trips comments losslessly.
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// `text` is the comment body with the leading marker and surrounding
/// whitespace removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub text: String,
    pub placement: CommentPlacement,
    pub marker: CommentMarker,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentMarker {
    Line,
    Doc,
}

/// Where a retained comment sits relative to its block's statements.
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
    Return {
        value: Option<Expression>,
        span: SourceSpan,
    },
    ReturnAbsent {
        span: SourceSpan,
    },
    Break {
        span: SourceSpan,
    },
    Continue {
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
    /// `if const name [: type] = place`: a saved-read existence guard that binds
    /// `name` only in the then block when `place` is present. The binding's type
    /// is the saved read's type; the optional annotation, parsed exactly as on
    /// `const`/`var`, names that type when written.
    IfConst {
        name: String,
        ty: Option<TypeRef>,
        value: Expression,
        then_block: Block,
        else_ifs: Vec<ElseIf>,
        else_block: Option<Block>,
        span: SourceSpan,
    },
    While {
        condition: Option<Expression>,
        body: Block,
        span: SourceSpan,
    },
    For {
        binding: ForBinding,
        iterable: Expression,
        /// The `by` step of a range header (`for x in lo..hi by step`), if one was
        /// written. Only a range iterable accepts a step; the checker rejects a step
        /// on any other iterable. `None` leaves the default step to the checker.
        step: Option<Expression>,
        body: Block,
        span: SourceSpan,
    },
    Transaction {
        body: Block,
        span: SourceSpan,
    },
    Try {
        body: Block,
        catch: Option<CatchClause>,
        span: SourceSpan,
    },
    /// A `match` over an enum-typed scrutinee: each arm names one member of the
    /// enum and holds the block to run when the scrutinee selects it. An arm is a
    /// member path *relative* to the scrutinee enum — a bare leaf (`bengal`), a
    /// qualified path (`tiger::bengal`), or a category (`tiger`, its whole
    /// subtree). The scrutinee supplies the enum, so an arm carries no enum prefix;
    /// a local enum's `match` has no wildcard arm. Exhaustiveness and member
    /// validity are checker rules.
    Match {
        scrutinee: Option<Expression>,
        arms: Vec<MatchArm>,
        span: SourceSpan,
    },
}

/// `path` is the arm's member path relative to the scrutinee enum, as written;
/// the checker walks it against that enum's member tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub path: Vec<String>,
    pub path_spans: Vec<SourceSpan>,
    pub block: Block,
    pub span: SourceSpan,
}

/// One `else if` clause of an `if` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElseIf {
    /// `None` when the condition text did not parse; the parser reports the error.
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
            | Self::Return { span, .. }
            | Self::ReturnAbsent { span }
            | Self::Break { span, .. }
            | Self::Continue { span, .. }
            | Self::Throw { span, .. }
            | Self::Expr { span, .. }
            | Self::If { span, .. }
            | Self::IfConst { span, .. }
            | Self::While { span, .. }
            | Self::For { span, .. }
            | Self::Transaction { span, .. }
            | Self::Try { span, .. }
            | Self::Match { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    /// `;;` doc lines above this parameter; empty for single-line lists, where
    /// parameter docs are not written.
    pub docs: Vec<String>,
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParam {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeRef {
    pub text: String,
    pub span: SourceSpan,
}

impl fmt::Display for TypeRef {
    // Verbatim source spelling, so the formatter re-emits it exactly; structured
    // resolution happens in marrow-schema.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}
