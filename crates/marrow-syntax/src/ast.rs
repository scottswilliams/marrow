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
    Alias(AliasDecl),
    Nominal(NominalDecl),
    Const(ConstDecl),
    Resource(ResourceDecl),
    Struct(StructDecl),
    Store(StoreDecl),
    Function(FunctionDecl),
    Enum(EnumDecl),
    Test(TestDecl),
}

/// A transparent type alias: `alias Name = Type`. The name denotes exactly its
/// target type — it mints no new identity and no constructor — so downstream
/// resolution expands it before classifying the annotation. Chains are allowed;
/// a cyclic chain is a check-time diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    /// `None` when the target type did not parse; the parser reports the error.
    pub ty: Option<TypeExpr>,
    pub span: SourceSpan,
}

/// A nominal type declaration: `type Name: base in lo..hi supports cap, ...`.
/// Unlike a transparent `alias`, the name mints a distinct type with its own
/// constructor; the `in` range constrains every value of the type and the
/// `supports` list names the capabilities that unlock arithmetic. The parser
/// captures the spelled parts; base admission, the literal-range rule, and the
/// closed capability set are checker rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NominalDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    /// `None` when the base type did not parse; the parser reports the error.
    pub base: Option<TypeExpr>,
    /// The `in` range expression as written (`0..150` or `0..=150`); `None` when
    /// missing or unparsable, which the parser reports.
    pub interval: Option<Expression>,
    /// The `supports` capability spellings, in source order.
    pub supports: Vec<SupportSpelling>,
    pub span: SourceSpan,
}

/// One spelled capability in a nominal declaration's `supports` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportSpelling {
    pub name: String,
    pub span: SourceSpan,
}

/// A `test "name"` declaration: a named, zero-argument, storeless body run by
/// `marrow test`. Its body is the only place the owned `assert` statement is
/// legal. The name is the decoded string-literal title; it is a report label, not
/// an export, interface, or durable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    pub body: Block,
    pub span: SourceSpan,
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
    /// Prefix `try <inner>`: propagate a `Result[T, E]`'s `err` out of the
    /// enclosing `Result`-returning function (same `E`), yielding the `ok` value.
    /// The parser produces this only as the top-level right-hand side of a
    /// statement; it is not a general sub-expression.
    Try {
        inner: Box<Expression>,
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
            | Self::Try { span, .. }
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
    /// The compound-assign operator a single `+=`, `-=`, `*=`, `/=`, or `%=`
    /// token spells, or `None` for any other token. Each compound operator is
    /// one lexer token, so this classifies that token directly; it is the single
    /// owner shared by the statement parser (which builds a compound assignment)
    /// and the expression parser (which rejects one reached in expression
    /// position).
    pub(crate) fn from_operator_token(kind: TokenKind) -> Option<Self> {
        match kind {
            TokenKind::PlusEqual => Some(Self::Add),
            TokenKind::MinusEqual => Some(Self::Subtract),
            TokenKind::StarEqual => Some(Self::Multiply),
            TokenKind::SlashEqual => Some(Self::Divide),
            TokenKind::PercentEqual => Some(Self::Remainder),
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

/// A dense product type: `struct Name` with an indented body of `name: Type`
/// fields. Unlike a `resource`, a struct is a non-durable value type — every
/// field is required, held inline, and copied by value — and it is constructed
/// with a named-only literal `Name(field: expr, ...)`. It shares the resource
/// member syntax, so groups, key parameters, and the `required` keyword parse
/// here; the checker rejects them, since a struct field is always the bare
/// `name: Type` form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructDecl {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    /// Declared generic type parameters, `[T, U supports order]`, empty for an
    /// ordinary monomorphic struct. A non-empty list makes the struct a template
    /// monomorphized at each `Name[Args]` use.
    pub type_params: Vec<TypeParamDecl>,
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
    /// Declared generic type parameters, `[T, U supports equality]`, empty for an
    /// ordinary monomorphic function. Each parameter names a type usable in the
    /// parameter, return, and local annotations of the body.
    pub type_params: Vec<TypeParamDecl>,
    pub params: Vec<ParamDecl>,
    pub return_type: Option<TypeExpr>,
    pub body: Block,
    pub span: SourceSpan,
}

/// One declared generic type parameter on a function: a name, optionally carrying
/// one closed constraint (`T supports equality` / `T supports order`). The parser
/// captures the spelling; the checker enforces that the constraint licenses the
/// corresponding operator over the parameter and revalidates it per application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeParamDecl {
    pub name: String,
    pub name_span: SourceSpan,
    pub constraint: Option<TypeConstraint>,
    pub span: SourceSpan,
}

/// The closed set of generic type-parameter constraints. `Equality` admits `==`/
/// `!=` over the parameter; `Order` admits `<`/`<=`/`>`/`>=` (and, being a superset
/// need, is checked independently). An unconstrained parameter admits neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConstraint {
    Equality,
    Order,
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
    /// Declared generic type parameters, `[T, U supports order]`, empty for an
    /// ordinary monomorphic enum. A non-empty list makes the enum a template
    /// monomorphized at each `Name[Args]` use.
    pub type_params: Vec<TypeParamDecl>,
    pub members: Vec<EnumMember>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// One enum member: a bare identifier, optionally carrying a parenthesized dense
/// payload (`circle(radius: int)`) and/or nested members under it. A `category`
/// member groups its descendants and is not selectable as a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumMember {
    pub docs: Vec<String>,
    pub name: String,
    pub name_span: SourceSpan,
    pub category: bool,
    /// The member's dense payload fields, in declaration order (empty for a
    /// payloadless member). Each is the bare `name: Type` form.
    pub payload: Vec<EnumPayloadField>,
    pub members: Vec<EnumMember>,
    pub comments: Vec<Comment>,
    pub span: SourceSpan,
}

/// One payload field of an enum member: a named, typed leaf carried by that
/// variant, as `name: Type`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumPayloadField {
    pub name: String,
    pub name_span: SourceSpan,
    pub ty: TypeExpr,
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
    /// `place name = ^root(key...)`: a function-local binding naming one concrete
    /// durable entry address. The key tuple in `place` is evaluated exactly once at
    /// the binding; the binding is immutable (a place is not re-assignable) and is
    /// not a first-class value. Later uses (`name.field`, `name = Record(...)`,
    /// `exists(name)`, `delete name`, `if const x = name`) resolve the operation
    /// through the pre-evaluated address. `place` is the durable entry address
    /// `^root(key...)`; the checker rejects a non-durable or field-projected target.
    PlaceBinding {
        name: String,
        name_span: SourceSpan,
        place: Expression,
        span: SourceSpan,
    },
    /// `unset place`: clear a local product's sparse field to absent. The `place`
    /// is a field access on a local (`r.note`); the checker rejects a required
    /// field, a non-field place, and a durable place.
    Unset {
        place: Expression,
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
    /// `assert <expr>`: the compiler/image/verifier/VM-owned test assertion. Legal
    /// only inside a `test` body; the checker rejects it elsewhere. Its `value` is a
    /// bool condition whose falsity faults the running test.
    Assert {
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
    /// B5 (parse-only): `if const a = e1 and const b = e2 and cond { … }` — one or
    /// more chained existence bindings and an optional trailing condition. Parsed so
    /// the grammar is complete; `marrow-compile` rejects it as `check.unsupported`
    /// until the form is adopted, so it never reaches the runtime.
    IfConstChain {
        bindings: Vec<IfConstBinding>,
        condition: Option<Expression>,
        then_block: Block,
        else_ifs: Vec<ElseIf>,
        else_block: Option<Block>,
        span: SourceSpan,
    },
    /// B6 (parse-only): let-else — `const x = e else <diverging>` or
    /// `var x = e else { … }`. Parsed so the grammar is complete; `marrow-compile`
    /// rejects it as `check.unsupported` until the form is adopted.
    LetElse {
        is_var: bool,
        name: String,
        ty: Option<TypeExpr>,
        value: Expression,
        else_block: Block,
        span: SourceSpan,
    },
    While {
        condition: Expression,
        body: Block,
        span: SourceSpan,
    },
    For {
        binding: ForBinding,
        /// Traversal direction of the head. `reversed` is a reserved keyword in the
        /// head slot between `in` and the iterable; everywhere else it is an ordinary
        /// identifier. A typed enum, not a `bool`: it selects behavior the runtime
        /// must consume.
        order: LoopOrder,
        iterable: Expression,
        /// The `by` step of a range header (`for x in lo..hi by step`), if one was
        /// written. Only a range iterable accepts a step; the checker rejects a step
        /// on any other iterable. `None` leaves the default step to the checker.
        step: Option<Expression>,
        /// The bounded durable-traversal clause `at most N [from f]` with its
        /// mandatory `on more` block, present only when the head carried `at most`.
        /// The checker requires it for a durable root/branch place and rejects it on a
        /// range or local-collection iterable.
        bound: Option<TraversalBound>,
        body: Block,
        span: SourceSpan,
    },
    Transaction {
        body: Block,
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
    /// The adjacent single-operation checked-arithmetic form, bound to a
    /// `const`/`var` or produced by `return`. It wraps one operation `op`; the `on`
    /// arms run when the operation faults and each must diverge. The parser captures
    /// `op` and the two optional arms; the checker owns which arms are required for
    /// the operation and that each arm diverges.
    Checked {
        bind: CheckedBind,
        op: Expression,
        out_of_range: Option<Block>,
        zero_divisor: Option<Block>,
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

/// The binding a `checked` form writes into: a fresh `const`/`var` (with an optional
/// type annotation) or a `return`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedBind {
    Const { name: String, ty: Option<TypeExpr> },
    Var { name: String, ty: Option<TypeExpr> },
    Return,
}

/// `path` is the arm's member path relative to the scrutinee enum, as written;
/// the checker walks it against that enum's member tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchArm {
    pub path: Vec<String>,
    pub path_spans: Vec<SourceSpan>,
    /// Positional payload bindings the arm introduces (`circle(r)` binds `r`),
    /// empty for a bare arm. The checker matches them against the member's payload
    /// arity and binds each to a fresh local in payload declaration order.
    pub bindings: Vec<ArmBinding>,
    pub block: Block,
    pub span: SourceSpan,
}

/// One existence binding in a chained `if const` head (B5). Parse-only; the
/// checker resolves the binding's type from the saved read once the form is
/// adopted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfConstBinding {
    pub name: String,
    pub ty: Option<TypeExpr>,
    pub value: Expression,
}

/// One positional payload binding in a `match` arm header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmBinding {
    pub name: String,
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

/// The loop variable(s) of a `for` statement: `for k in ...`,
/// `for k, v in ...`, or the composite-layer `for c0, c1, .., v in ...`. The
/// binding is a non-empty name vector; the parser guarantees `names` holds at
/// least one entry. A single name binds the key; additional names bind the
/// remaining key columns and the leaf value, per the loop-head arity rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForBinding {
    pub names: Vec<ForName>,
}

/// One bound name of a `for` head, carrying its own span for per-name arity
/// diagnostics and editor cursors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForName {
    pub name: String,
    pub span: SourceSpan,
}

/// Traversal direction of a `for` head. `Reversed` walks a layer's addresses,
/// a local collection's keys, or a range in descending order; `Forward` is the
/// default ascending walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopOrder {
    Forward,
    Reversed,
}

/// The bounded durable-traversal clause of a `for` head: `at most N [from f]`
/// paired with its `on more` block. `at most N` caps how many immediate keys the
/// traversal freezes; the optional inclusive `from f` starts the walk at or after a
/// lower-bound key; the `on more` block runs when a further key existed beyond the
/// frozen `N` and the frozen bodies all completed normally. The checker enforces that
/// `N` is a positive compile-time literal within the traversal ceiling, that `on more`
/// is present, and that the iterable is a durable root or single-level branch place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalBound {
    /// The `at most N` limit expression.
    pub limit: Expression,
    /// The inclusive `from f` lower-bound key expression, if written.
    pub from: Option<Expression>,
    /// The `on more` block. `None` when `at most` appeared with no trailing `on more`
    /// block — the checker reports the missing arm.
    pub on_more: Option<Block>,
}

impl Statement {
    pub fn span(&self) -> SourceSpan {
        match self {
            Self::Const { span, .. }
            | Self::Var { span, .. }
            | Self::Assign { span, .. }
            | Self::CompoundAssign { span, .. }
            | Self::Delete { span, .. }
            | Self::PlaceBinding { span, .. }
            | Self::Unset { span, .. }
            | Self::Return { span, .. }
            | Self::Break { span, .. }
            | Self::Continue { span, .. }
            | Self::Assert { span, .. }
            | Self::Expr { span, .. }
            | Self::If { span, .. }
            | Self::IfConst { span, .. }
            | Self::IfConstChain { span, .. }
            | Self::LetElse { span, .. }
            | Self::While { span, .. }
            | Self::For { span, .. }
            | Self::Transaction { span, .. }
            | Self::Match { span, .. }
            | Self::Checked { span, .. }
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
    /// Empty for an ordinary scalar, resource, collection, or identity parameter,
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
/// generic-application `Head[..]`, `Id(^root)`, and trailing-`?` forms here, so downstream
/// consumers match on this node instead of re-reading the source spelling. The
/// grammar of type spellings has exactly one owner: the type parser that builds
/// this node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    /// A name that is not a recognized special form: a scalar spelling, `unknown`,
    /// an enum or resource name, a qualified name, or an unresolvable spelling.
    /// `text` is the whitespace-free source spelling; classifying it as a scalar,
    /// `unknown`, or a named type is a resolution concern that needs project
    /// knowledge, so it stays with the semantic owner.
    Name { text: String, span: SourceSpan },
    /// `Id(^root)`, a saved-store identity type.
    Identity(IdentityTypeExpr),
    /// `T?`, an optional value type.
    Optional {
        inner: Box<TypeExpr>,
        span: SourceSpan,
    },
    /// A generic type application `Head[Arg, ...]`. The head is any identifier: the
    /// toolchain generics
    /// `Option[T]`/`Result[T, E]`/`List[T]`/`Map[K, V]` or a user-declared generic
    /// `struct`/`enum` template. `head` is the applied name and `args` its type
    /// arguments in source order; the semantic owner resolves the head.
    Apply {
        head: String,
        args: Vec<TypeExpr>,
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
            | TypeExpr::Optional { span, .. }
            | TypeExpr::Apply { span, .. } => *span,
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
            TypeExpr::Identity(identity) => write!(f, "Id(^{})", identity.root),
            TypeExpr::Optional { inner, .. } => write!(f, "{inner}?"),
            // The canonical spelling separates arguments with `", "`. Any source
            // spacing parses to the same node, so this is idempotent and the digest
            // it feeds is stable across reformatting.
            TypeExpr::Apply { head, args, .. } => {
                write!(f, "{head}[")?;
                for (index, arg) in args.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                f.write_str("]")
            }
        }
    }
}
