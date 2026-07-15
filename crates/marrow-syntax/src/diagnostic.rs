//! The diagnostic surface shared across the toolchain: error/warning records,
//! the severity scale, the `Diagnose` trait the CLI renders, and source spans.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub reason: DiagnosticReason,
    pub severity: Severity,
    pub message: String,
    pub help: Option<String>,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticReason {
    Lexer(LexerDiagnosticReason),
    Parser(ParseDiagnosticReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexerDiagnosticReason {
    IndentationMismatch,
    ObsoleteOperator(ObsoleteOperator),
    ReservedTilde,
    TabIndentation,
    UnexpectedCharacter(char),
    UnterminatedInterpolationExpression,
    UnterminatedInterpolationString,
    UnterminatedString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObsoleteOperator {
    AndAnd,
    Bang,
    Hash,
    OrOr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseDiagnosticReason {
    ConstRequiresValue,
    DocCommentBeforeParameter,
    /// A `;;` doc comment with no following declaration, member, or parameter to
    /// attach to: in a statement position, or dangling at end of file or body.
    DocCommentWithoutTarget,
    EmptyIndexArguments,
    EmptyKeyParameters,
    EnumMemberMustBeBareName,
    EnumNeedsMember,
    /// A compound-assign operator (`+=`, `-=`, `*=`, `/=`, `%=`) reached in
    /// expression position, as in `a += b += c`: assignment does not chain and
    /// is not an expression.
    CompoundAssignInExpression,
    /// A compound-assign operator written with a space before the `=`
    /// (`x * = y`). Each compound operator is a single token, so the split
    /// spelling is rejected; write it compact (`x *= y`).
    SplitCompoundAssign,
    /// A bare `=` left in expression position, the common `=`-for-`==` mistake.
    EqualsInExpression,
    Expected(ExpectedSyntax),
    IndexOutsideStoreBody,
    InvalidVisibility,
    KeywordExpression,
    KeywordFieldName,
    /// A reserved word used as a segment of a `use` or `module` path, where the
    /// grammar admits only identifiers.
    KeywordPathSegment,
    LateModuleDeclaration,
    /// A `checked` form's arm header is not `on out_of_range`/`on zero_divisor`.
    CheckedArm,
    MatchArmMemberPath,
    NestingLimit,
    /// A second operator on a non-associative level (`==`/`!=`/`</`is`/`??`),
    /// which the grammar does not chain.
    NonAssociativeOperator,
    PositionalArgumentAfterNamed,
    Reserved(ReservedSyntax),
    ResourceMemberInStoreBody,
    UnexpectedIndentation,
    Unsupported(UnsupportedSyntax),
}

impl ParseDiagnosticReason {
    /// The dotted code a declaration-parser diagnostic carrying this reason
    /// renders under. Nesting overflow is a `check.nesting_limit` finding
    /// wherever the front end raises it, so it surfaces alongside the type-check
    /// findings the operator reads; every other declaration parse error is
    /// `parse.syntax`.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::NestingLimit => crate::NESTING_LIMIT,
            _ => crate::PARSE_SYNTAX,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedSyntax {
    AliasName,
    AliasType,
    CloseParen,
    Comma,
    CheckedBody,
    ConstName,
    ConstType,
    Declaration,
    DefaultValue,
    EnumBody,
    EnumHeader,
    EnumName,
    EvolveBody,
    EvolveStep,
    EvolveTargetPath,
    Expression,
    FieldType,
    FunctionBody,
    FunctionHeader,
    FunctionName,
    FunctionParameterList,
    FunctionReturnType,
    ImportName,
    IndexArgumentList,
    IndexFieldPath,
    IndexName,
    IndexTail,
    InterpolationHoleEnd,
    KeyName,
    KeyParameterList,
    KeyType,
    MatchBody,
    ModuleName,
    ParameterName,
    ParameterType,
    ResourceBody,
    ResourceHeader,
    ResourceMemberName,
    ResourceMemberSyntax,
    ResourceName,
    SavedRootName,
    Statement,
    StoreRoot,
    StoreResourceName,
    TestBody,
    TestName,
    TransformBody,
    VariableName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservedSyntax {
    LockStatement,
    MergeStatement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedSyntax {
    BracketCollectionLiterals,
    Finally,
    LoopLabels,
    ParameterModes,
    ParameterDefaults,
    QuotedFieldSegments,
    UserDefinedGenerics,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}: {}: {}: {}",
            self.span.line,
            self.span.column,
            self.severity.as_str(),
            self.code,
            self.message
        )
    }
}

impl Diagnose for Diagnostic {
    fn code(&self) -> &str {
        self.code
    }
    fn message(&self) -> &str {
        &self.message
    }
    fn severity(&self) -> Severity {
        self.severity
    }
    fn help(&self) -> Option<&str> {
        self.help.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

/// The common error surface the CLI renders uniformly over `&dyn Diagnose`. The
/// source-span shape stays per source: a parse span, a project line/column, and
/// a path-located finding are not the same object.
pub trait Diagnose {
    fn code(&self) -> &str;
    fn message(&self) -> &str;
    fn severity(&self) -> Severity {
        Severity::Error
    }
    fn help(&self) -> Option<&str> {
        None
    }
    fn kind(&self) -> &str {
        marrow_codes::kind_for_code(self.code())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}
