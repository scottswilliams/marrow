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
    EmptyIndexArguments,
    EmptyKeyParameters,
    EnumMemberMustBeBareName,
    EnumNeedsMember,
    Expected(ExpectedSyntax),
    IndexOutsideStoreBody,
    InvalidVisibility,
    KeywordExpression,
    KeywordFieldName,
    LateModuleDeclaration,
    MatchArmMemberPath,
    NestingLimit,
    PositionalArgumentAfterNamed,
    Reserved(ReservedSyntax),
    ResourceMemberInStoreBody,
    UnexpectedIndentation,
    Unsupported(UnsupportedSyntax),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpectedSyntax {
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
    KeyName,
    KeyParameterList,
    KeyType,
    MatchBody,
    ModuleName,
    ParameterName,
    ParameterType,
    ResourceBody,
    ResourceMemberName,
    ResourceMemberSyntax,
    ResourceName,
    SavedRootName,
    Statement,
    StoreRoot,
    StoreResourceName,
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
    Finally,
    LoopLabels,
    ParameterModes,
    ParameterDefaults,
    QuotedFieldSegments,
    TypeAliases,
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
        kind_for_code(self.code())
    }
}

/// The broad `kind` category for a dotted error code. The first segment is not
/// always the kind name (`run.*` is `runtime`), so the mapping is explicit.
pub fn kind_for_code(code: &str) -> &'static str {
    match code.split('.').next().unwrap_or("") {
        "parse" => "parse",
        "check" | "schema" => "check",
        "run" | "value" => "runtime",
        "store" => "storage",
        "io" => "io",
        // Configuration and project-discovery failures are tooling errors.
        _ => "tooling",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceSpan {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}

#[cfg(test)]
mod tests {
    use super::kind_for_code;

    #[test]
    fn value_codes_are_runtime_diagnostics() {
        assert_eq!(kind_for_code("value.range"), "runtime");
    }

    #[test]
    fn unknown_code_families_are_tooling_diagnostics() {
        assert_eq!(kind_for_code("unknown.family"), "tooling");
    }
}
