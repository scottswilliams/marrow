//! Abstract syntax tree: the data types produced by parsing a Marrow source
//! file, together with their small accessor impls.

use std::fmt;

use crate::{Diagnostic, Severity, SourceSpan, TokenKind};

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
    pub ty: Option<TypeExpr>,
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
    /// The empty-optional primary value `absent`: assignable to any `T?` place and
    /// inert until resolved.
    Absent { span: SourceSpan },
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
    /// A span of source the parser could not structure as an expression. Total
    /// parsing yields this node in place of a dropped operand so every parse
    /// produces a tree; it always travels with a `parse.syntax` diagnostic at its
    /// span, and semantic processing is gated on `!ParsedSource::has_errors`, so a
    /// checker or runtime never resolves an `Error`.
    Error { span: SourceSpan },
}

impl Expression {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Literal { span, .. }
            | Self::Name { span, .. }
            | Self::SavedRoot { span, .. }
            | Self::Absent { span }
            | Self::Call { span, .. }
            | Self::Field { span, .. }
            | Self::OptionalField { span, .. }
            | Self::Unary { span, .. }
            | Self::Binary { span, .. }
            | Self::Range { span, .. }
            | Self::Interpolation { span, .. }
            | Self::Error { span } => *span,
        }
    }

    /// Whether this node is the total-parser's error placeholder. Recovery uses it
    /// to propagate a failure upward without emitting a second diagnostic.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompoundAssignOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

impl CompoundAssignOp {
    /// The compound-assign operator a token spells when it sits immediately
    /// before the `=` of `+=`, `-=`, `*=`, `/=`, `%=`, or `None` for any other
    /// token. This is the single owner of the operator-token classification,
    /// shared by the statement parser (which splits a compound assignment) and
    /// the expression parser (which rejects one reached in expression position).
    pub(crate) fn from_operator_token(kind: TokenKind) -> Option<Self> {
        match kind {
            TokenKind::Plus => Some(Self::Add),
            TokenKind::Minus => Some(Self::Subtract),
            TokenKind::Star => Some(Self::Multiply),
            TokenKind::Slash => Some(Self::Divide),
            TokenKind::Percent => Some(Self::Remainder),
            _ => None,
        }
    }

    pub fn binary(self) -> BinaryOp {
        match self {
            Self::Add => BinaryOp::Add,
            Self::Subtract => BinaryOp::Subtract,
            Self::Multiply => BinaryOp::Multiply,
            Self::Divide => BinaryOp::Divide,
            Self::Remainder => BinaryOp::Remainder,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+=",
            Self::Subtract => "-=",
            Self::Multiply => "*=",
            Self::Divide => "/=",
            Self::Remainder => "%=",
        }
    }
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
        name_spans: Vec<SourceSpan>,
        span: SourceSpan,
    },
    Collection {
        target: SurfaceTarget,
        alias: String,
        span: SourceSpan,
    },
    Action {
        function: Vec<String>,
        function_span: SourceSpan,
        alias: String,
        span: SourceSpan,
    },
    Read {
        function: Vec<String>,
        function_span: SourceSpan,
        alias: String,
        span: SourceSpan,
    },
    Create {
        names: Vec<String>,
        name_spans: Vec<SourceSpan>,
        span: SourceSpan,
    },
    Update {
        names: Vec<String>,
        name_spans: Vec<SourceSpan>,
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
            | Self::Read { span, .. }
            | Self::Create { span, .. }
            | Self::Update { span, .. }
            | Self::Delete { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceTarget {
    Root {
        root: String,
        span: SourceSpan,
    },
    Index {
        root: String,
        index: String,
        span: SourceSpan,
    },
    IndexRange {
        root: String,
        index: String,
        span: SourceSpan,
    },
}

impl SurfaceTarget {
    /// The span of the `^target` token(s), so a checker rejection points at the
    /// offending target rather than column 1 of the `collection` line.
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Root { span, .. } | Self::Index { span, .. } | Self::IndexRange { span, .. } => {
                *span
            }
        }
    }
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
    pub ty: TypeExpr,
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
    /// The source span of each argument, parallel to `args`, so a per-argument
    /// diagnostic points at the offending path rather than the whole `index` line.
    pub arg_spans: Vec<SourceSpan>,
    pub unique: bool,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDecl {
    pub docs: Vec<String>,
    pub public: bool,
    pub name: String,
    pub params: Vec<ParamDecl>,
    pub return_type: Option<TypeExpr>,
    pub body: Block,
    pub span: SourceSpan,
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
        ty: Option<TypeExpr>,
        value: Expression,
        span: SourceSpan,
    },
    Var {
        name: String,
        keys: Vec<KeyParam>,
        ty: Option<TypeExpr>,
        value: Option<Expression>,
        span: SourceSpan,
    },
    Assign {
        target: Expression,
        value: Expression,
        span: SourceSpan,
    },
    CompoundAssign {
        target: Expression,
        op: CompoundAssignOp,
        op_span: SourceSpan,
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
        condition: Expression,
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
        ty: Option<TypeExpr>,
        value: Expression,
        then_block: Block,
        else_ifs: Vec<ElseIf>,
        else_block: Option<Block>,
        span: SourceSpan,
    },
    While {
        condition: Expression,
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
        scrutinee: Expression,
        arms: Vec<MatchArm>,
        span: SourceSpan,
    },
    /// A statement line the parser could not structure. Total parsing yields this
    /// node in place of a dropped line so every body parses to a statement list;
    /// it always travels with a `parse.syntax` diagnostic at its span, and
    /// semantic processing is gated on `!ParsedSource::has_errors`.
    Error {
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
    /// `Expression::Error` when the condition text did not parse; the parser
    /// reports the error at that span.
    pub condition: Expression,
    pub block: Block,
}

/// The `catch name: Error` clause of a `try` statement. `ty` is the optional
/// type annotation on the bound error value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchClause {
    pub name: String,
    pub ty: Option<TypeExpr>,
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
            | Self::CompoundAssign { span, .. }
            | Self::Delete { span, .. }
            | Self::Return { span, .. }
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
            | Self::Match { span, .. }
            | Self::Error { span } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDecl {
    /// `;;` doc lines above this parameter; empty for single-line lists, where
    /// parameter docs are not written.
    pub docs: Vec<String>,
    pub name: String,
    /// Key parameters when the parameter is a local keyed collection
    /// (`scores(player: string): int`), spelled like the local declaration head.
    /// Empty for an ordinary scalar, resource, sequence, or identity parameter,
    /// where `ty` alone is the parameter type.
    pub keys: Vec<KeyParam>,
    pub ty: TypeExpr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyParam {
    pub name: String,
    pub ty: TypeExpr,
}

/// A type annotation, parsed once into its structure. The parser classifies the
/// `sequence[T]`, `Id(^root)`, and trailing-`?` forms here, so `marrow-schema` and
/// the checker match on this node instead of re-reading the source spelling. The
/// grammar of type spellings has exactly one owner: the type parser that builds
/// this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    /// A name that is not a recognized special form: a scalar spelling, `unknown`,
    /// an enum or resource name, a qualified name, or an unresolvable spelling.
    /// `text` is the whitespace-free source spelling; classifying it as a scalar,
    /// `unknown`, or a named type is a resolution concern that needs project
    /// knowledge, so it stays in `marrow-schema` and the checker.
    Name { text: String, span: SourceSpan },
    /// `sequence[T]` element-type sugar.
    Sequence {
        element: Box<TypeExpr>,
        span: SourceSpan,
    },
    /// `Id(^root)`, a saved-store identity type.
    Identity(IdentityTypeExpr),
    /// `T?`, an optional value type.
    Optional {
        inner: Box<TypeExpr>,
        span: SourceSpan,
    },
}

/// The parts of an `Id(^root)` identity annotation, spans included so tooling can
/// address the constructor and the saved root without re-lexing the spelling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityTypeExpr {
    /// The saved-store root the identity references.
    pub root: String,
    /// The `Id` constructor keyword.
    pub keyword_span: SourceSpan,
    /// The `^` of the saved-root reference.
    pub caret_span: SourceSpan,
    /// The root name identifier.
    pub root_span: SourceSpan,
    /// The whole `Id(^root)` annotation.
    pub span: SourceSpan,
}

impl TypeExpr {
    /// The source span of the whole annotation.
    pub fn span(&self) -> SourceSpan {
        match self {
            TypeExpr::Name { span, .. }
            | TypeExpr::Sequence { span, .. }
            | TypeExpr::Optional { span, .. } => *span,
            TypeExpr::Identity(identity) => identity.span,
        }
    }
}

impl fmt::Display for TypeExpr {
    // The canonical, whitespace-free source spelling. The formatter re-emits it
    // exactly and the durable digest hashes it, so this is the inverse of the type
    // parser: a spelling parsed and re-rendered is byte-identical.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeExpr::Name { text, .. } => f.write_str(text),
            TypeExpr::Sequence { element, .. } => write!(f, "sequence[{element}]"),
            TypeExpr::Identity(identity) => write!(f, "Id(^{})", identity.root),
            TypeExpr::Optional { inner, .. } => write!(f, "{inner}?"),
        }
    }
}
