//! The diagnostic surface shared across the toolchain: error/warning records,
//! the severity scale, the `Diagnose` trait the CLI renders, source spans, and
//! the bounded per-entry-point collector every syntax producer writes through.

use std::fmt;

/// The most diagnostic rows one entry point retains before it discards the
/// payload and reports a typed [`SyntaxDiagnosticLimit::Count`]. Shared with
/// the compiler's collector ceiling; a cross-crate drift test pins equality.
pub const SYNTAX_DIAGNOSTIC_COUNT_LIMIT: usize = 4096;

/// The most retained owned payload bytes (message + help + reason-owned bytes)
/// one entry point holds before it discards the payload and reports a typed
/// [`SyntaxDiagnosticLimit::OwnedBytes`]. A logical initialized-payload budget,
/// never a bound on allocation capacity.
pub const SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT: usize = 1024 * 1024;

/// The bounded result of one syntax entry point: either the complete sorted
/// payload, or the typed limit that discarded it. Construction is private to
/// the collector; consumers read the copyable summary and the typed payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxDiagnostics {
    state: SyntaxDiagnosticState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SyntaxDiagnosticState {
    Complete {
        summary: SyntaxDiagnosticSummary,
        payload: CompleteSyntaxDiagnostics,
    },
    Limited {
        summary: SyntaxDiagnosticSummary,
        limit: SyntaxDiagnosticLimit,
    },
}

impl SyntaxDiagnostics {
    /// The copyable count/owned-byte totals. Exact for a complete result;
    /// saturated at limit plus one for a limited one. Every row is an error,
    /// so a non-zero count is the has-errors fact.
    pub fn summary(&self) -> SyntaxDiagnosticSummary {
        match &self.state {
            SyntaxDiagnosticState::Complete { summary, .. }
            | SyntaxDiagnosticState::Limited { summary, .. } => *summary,
        }
    }

    /// Borrow the complete payload, or the typed limit that discarded it.
    pub fn as_complete(&self) -> Result<&CompleteSyntaxDiagnostics, SyntaxDiagnosticLimit> {
        match &self.state {
            SyntaxDiagnosticState::Complete { payload, .. } => Ok(payload),
            SyntaxDiagnosticState::Limited { limit, .. } => Err(*limit),
        }
    }

    /// Consume into the complete payload, or the typed limit that discarded it.
    pub fn into_complete(self) -> Result<CompleteSyntaxDiagnostics, SyntaxDiagnosticLimit> {
        match self.state {
            SyntaxDiagnosticState::Complete { payload, .. } => Ok(payload),
            SyntaxDiagnosticState::Limited { limit, .. } => Err(limit),
        }
    }
}

/// The copyable totals of a bounded diagnostic result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntaxDiagnosticSummary {
    count: usize,
    owned_bytes: usize,
}

impl SyntaxDiagnosticSummary {
    /// Diagnostic rows produced; saturated at the count ceiling plus one once
    /// limited.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Owned payload bytes produced (message + help + reason-owned bytes);
    /// saturated at the byte ceiling plus one once limited.
    pub fn owned_bytes(&self) -> usize {
        self.owned_bytes
    }
}

/// The typed ceiling that discarded a diagnostic payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxDiagnosticLimit {
    Count { limit: usize },
    OwnedBytes { limit: usize },
}

/// The complete, position-sorted diagnostic payload of one entry point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteSyntaxDiagnostics(Box<[Diagnostic]>);

impl CompleteSyntaxDiagnostics {
    pub fn as_slice(&self) -> &[Diagnostic] {
        &self.0
    }

    /// Consume into the owned rows, so a downstream envelope (the compiler's
    /// syntax bridge) moves the payload instead of cloning it row by row.
    pub fn into_boxed_slice(self) -> Box<[Diagnostic]> {
        self.0
    }

    /// The sole constructor of the nonempty wrapper: `Some` exactly when the
    /// payload holds at least one row.
    pub fn into_non_empty(self) -> Option<NonEmptyCompleteSyntaxDiagnostics> {
        if self.0.is_empty() {
            None
        } else {
            Some(NonEmptyCompleteSyntaxDiagnostics(self))
        }
    }
}

/// A complete payload proven nonempty by [`CompleteSyntaxDiagnostics::into_non_empty`],
/// the typed evidence a parse-invalid refusal carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmptyCompleteSyntaxDiagnostics(CompleteSyntaxDiagnostics);

impl NonEmptyCompleteSyntaxDiagnostics {
    pub fn as_slice(&self) -> &[Diagnostic] {
        self.0.as_slice()
    }
}

/// A finalized syntax finding: message, help, and reason are fixed at
/// construction (A8) and the sole constructor fixes [`Severity::Error`], so a
/// collector payload is provably error-only without a severity check.
pub(crate) struct SyntaxError(Diagnostic);

impl SyntaxError {
    /// The code is derived from the typed reason, never passed beside it: the
    /// reason owns the mapping, so a code/reason mismatch is unrepresentable.
    pub(crate) fn new(
        reason: DiagnosticReason,
        message: impl Into<String>,
        help: Option<String>,
        span: SourceSpan,
    ) -> Self {
        Self(Diagnostic {
            code: reason.code(),
            reason,
            severity: Severity::Error,
            message: message.into(),
            help,
            span,
        })
    }

    /// The retained owned payload bytes this row charges against
    /// [`SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT`].
    fn owned_bytes(&self) -> usize {
        self.0.message.len()
            + self.0.help.as_deref().map_or(0, str::len)
            + self.0.reason.owned_bytes()
    }
}

impl DiagnosticReason {
    /// The dotted code a diagnostic carrying this reason renders under. Every
    /// lexical finding is `parse.syntax`; a declaration-parser reason maps
    /// through [`ParseDiagnosticReason::code`].
    fn code(&self) -> &'static str {
        match self {
            Self::Lexer(_) => crate::PARSE_SYNTAX,
            Self::Parser(reason) => reason.code(),
        }
    }

    /// Heap bytes owned by the reason payload. Every current variant carries
    /// only `Copy` data, so the charge is zero; the exhaustive matches force a
    /// future owning variant to declare its charge here.
    fn owned_bytes(&self) -> usize {
        match self {
            Self::Lexer(reason) => match reason {
                LexerDiagnosticReason::IndentationMismatch
                | LexerDiagnosticReason::ObsoleteOperator(_)
                | LexerDiagnosticReason::ReservedTilde
                | LexerDiagnosticReason::TabIndentation
                | LexerDiagnosticReason::UnexpectedCharacter(_)
                | LexerDiagnosticReason::UnterminatedInterpolationExpression
                | LexerDiagnosticReason::UnterminatedInterpolationString
                | LexerDiagnosticReason::UnterminatedString => 0,
            },
            Self::Parser(reason) => match reason {
                ParseDiagnosticReason::ConstRequiresValue
                | ParseDiagnosticReason::DocCommentBeforeParameter
                | ParseDiagnosticReason::DocCommentWithoutTarget
                | ParseDiagnosticReason::EmptyIndexArguments
                | ParseDiagnosticReason::EmptyKeyParameters
                | ParseDiagnosticReason::EnumMemberMustBeBareName
                | ParseDiagnosticReason::EnumNeedsMember
                | ParseDiagnosticReason::CompoundAssignInExpression
                | ParseDiagnosticReason::SplitCompoundAssign
                | ParseDiagnosticReason::EqualsInExpression
                | ParseDiagnosticReason::Expected(_)
                | ParseDiagnosticReason::IndexOutsideStoreBody
                | ParseDiagnosticReason::InvalidVisibility
                | ParseDiagnosticReason::KeywordExpression
                | ParseDiagnosticReason::KeywordFieldName
                | ParseDiagnosticReason::KeywordPathSegment
                | ParseDiagnosticReason::LateModuleDeclaration
                | ParseDiagnosticReason::CheckedArm
                | ParseDiagnosticReason::MatchArmMemberPath
                | ParseDiagnosticReason::NamedKeyArgument
                | ParseDiagnosticReason::NestingLimit
                | ParseDiagnosticReason::NonAssociativeOperator
                | ParseDiagnosticReason::PositionalArgumentAfterNamed
                | ParseDiagnosticReason::Reserved(_)
                | ParseDiagnosticReason::ResourceMemberInStoreBody
                | ParseDiagnosticReason::UnexpectedBlock
                | ParseDiagnosticReason::UnfixedDurationUnit
                | ParseDiagnosticReason::Unsupported(_) => 0,
            },
        }
    }
}

/// The producer that wrote a row, ranking lexer rows before parser rows on a
/// position tie so recovery order is emission-stable.
#[derive(Clone, Copy)]
enum SyntaxProducer {
    Lexer,
    Parser,
}

impl SyntaxProducer {
    fn rank(self) -> u8 {
        match self {
            Self::Lexer => 0,
            Self::Parser => 1,
        }
    }
}

struct SyntaxRow {
    producer_rank: u8,
    ordinal: usize,
    error: SyntaxError,
}

/// The one live diagnostic owner of a syntax entry point. Producers write
/// through scoped [`SyntaxSink`] borrows; `finish` consumes the owner exactly
/// once into the opaque bounded result. Retention is bounded by the typed
/// Count/OwnedBytes ceilings with count-before-bytes precedence; crossing a
/// ceiling discards the payload destructively while the saturated totals keep
/// accumulating (a Bytes limit may strengthen to Count, never the reverse, and
/// the payload never re-materializes).
pub(crate) struct SyntaxDiagnosticCollector {
    state: CollectorState,
    /// Per-producer ordinals, advanced on every push — even after Limited — so
    /// row identity stays emission-stable regardless of retention.
    ordinals: [usize; 2],
}

enum CollectorState {
    Retaining {
        owned_bytes: usize,
        rows: Vec<SyntaxRow>,
    },
    Limited {
        count: usize,
        owned_bytes: usize,
        limit: SyntaxDiagnosticLimit,
    },
}

impl SyntaxDiagnosticCollector {
    pub(crate) fn new() -> Self {
        Self {
            state: CollectorState::Retaining {
                owned_bytes: 0,
                rows: Vec::new(),
            },
            ordinals: [0; 2],
        }
    }

    /// A scoped sink for lexer-produced rows; the borrow must end before a
    /// parser sink is taken.
    pub(crate) fn lexer_sink(&mut self) -> SyntaxSink<'_> {
        SyntaxSink {
            collector: self,
            producer: SyntaxProducer::Lexer,
        }
    }

    /// A scoped sink for parser-produced rows over the same owner.
    pub(crate) fn parser_sink(&mut self) -> SyntaxSink<'_> {
        SyntaxSink {
            collector: self,
            producer: SyntaxProducer::Parser,
        }
    }

    fn push(&mut self, producer: SyntaxProducer, error: SyntaxError) {
        let rank = producer.rank();
        let ordinal = self.ordinals[rank as usize];
        self.ordinals[rank as usize] += 1;
        let row_bytes = error.owned_bytes();
        match &mut self.state {
            CollectorState::Retaining { owned_bytes, rows } => {
                let new_count = rows.len() + 1;
                let new_bytes = owned_bytes.saturating_add(row_bytes);
                if new_count > SYNTAX_DIAGNOSTIC_COUNT_LIMIT {
                    // Count is checked first, so it wins a simultaneous
                    // crossing. The discard is destructive: the rows drop here
                    // and never re-materialize.
                    self.state = CollectorState::Limited {
                        count: new_count.min(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1),
                        owned_bytes: new_bytes.min(SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT + 1),
                        limit: SyntaxDiagnosticLimit::Count {
                            limit: SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
                        },
                    };
                } else if new_bytes > SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT {
                    self.state = CollectorState::Limited {
                        count: new_count,
                        owned_bytes: new_bytes.min(SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT + 1),
                        limit: SyntaxDiagnosticLimit::OwnedBytes {
                            limit: SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT,
                        },
                    };
                } else {
                    *owned_bytes = new_bytes;
                    rows.push(SyntaxRow {
                        producer_rank: rank,
                        ordinal,
                        error,
                    });
                }
            }
            CollectorState::Limited {
                count,
                owned_bytes,
                limit,
            } => {
                *count = (*count + 1).min(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
                *owned_bytes = owned_bytes
                    .saturating_add(row_bytes)
                    .min(SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT + 1);
                if matches!(limit, SyntaxDiagnosticLimit::OwnedBytes { .. })
                    && *count > SYNTAX_DIAGNOSTIC_COUNT_LIMIT
                {
                    *limit = SyntaxDiagnosticLimit::Count {
                        limit: SYNTAX_DIAGNOSTIC_COUNT_LIMIT,
                    };
                }
            }
        }
    }

    /// Consume the owner into its bounded result: the stable
    /// `(line, start, producer rank, ordinal)` sort makes position dominate
    /// with lexer-first ties, and unwraps each row's error-fixed diagnostic.
    /// This is the one ordering owner for every entry point, so a lexing-only
    /// entry point reports position order too: raw emission order is not
    /// observable anywhere on the public surface.
    pub(crate) fn finish(self) -> SyntaxDiagnostics {
        match self.state {
            CollectorState::Retaining {
                owned_bytes,
                mut rows,
            } => {
                rows.sort_by_key(|row| {
                    (
                        row.error.0.span.line,
                        row.error.0.span.start_byte,
                        row.producer_rank,
                        row.ordinal,
                    )
                });
                let summary = SyntaxDiagnosticSummary {
                    count: rows.len(),
                    owned_bytes,
                };
                let payload =
                    CompleteSyntaxDiagnostics(rows.into_iter().map(|row| row.error.0).collect());
                SyntaxDiagnostics {
                    state: SyntaxDiagnosticState::Complete { summary, payload },
                }
            }
            CollectorState::Limited {
                count,
                owned_bytes,
                limit,
            } => SyntaxDiagnostics {
                state: SyntaxDiagnosticState::Limited {
                    summary: SyntaxDiagnosticSummary { count, owned_bytes },
                    limit,
                },
            },
        }
    }
}

/// Where a producer submits finalized syntax errors: the live scoped borrow of
/// the entry point's collector, or a discarding sink for silent probes.
pub(crate) trait SyntaxErrorSink {
    fn push(&mut self, error: SyntaxError);
}

/// A scoped producer borrow of the one live collector.
pub(crate) struct SyntaxSink<'c> {
    collector: &'c mut SyntaxDiagnosticCollector,
    producer: SyntaxProducer,
}

impl SyntaxSink<'_> {
    pub(crate) fn push(&mut self, error: SyntaxError) {
        self.collector.push(self.producer, error);
    }
}

impl SyntaxErrorSink for SyntaxSink<'_> {
    fn push(&mut self, error: SyntaxError) {
        Self::push(self, error);
    }
}

/// The sink a silent probe writes to: rows vanish without touching any
/// collector total. Never a dummy vector.
pub(crate) struct DiscardingSyntaxErrorSink;

impl SyntaxErrorSink for DiscardingSyntaxErrorSink {
    fn push(&mut self, _error: SyntaxError) {}
}

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
    /// A `///` doc comment with no following declaration, member, or parameter to
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
    /// A named argument inside a keyed-access bracket group (`^books[id: 3]`). Key
    /// arguments are an ordered positional tuple; a keyed access takes no named
    /// argument.
    NamedKeyArgument,
    NestingLimit,
    /// A second operator on a non-associative level (`==`/`!=`/`</`is`/`??`),
    /// which the grammar does not chain.
    NonAssociativeOperator,
    PositionalArgumentAfterNamed,
    Reserved(ReservedSyntax),
    ResourceMemberInStoreBody,
    /// A `{ … }` block appears where the grammar admits none — only compound
    /// statements and body-bearing declarations introduce blocks.
    UnexpectedBlock,
    /// A duration word literal spelled with a month or year (`1 month`, `2 years`),
    /// which have no fixed span. The fixed-unit set runs second(s) through week(s).
    UnfixedDurationUnit,
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
    CloseBrace,
    CloseBracket,
    CloseParen,
    CloseTypeArguments,
    Comma,
    CheckedBody,
    ConstName,
    ConstType,
    Declaration,
    EnumBody,
    EnumHeader,
    EnumName,
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
    NominalBase,
    NominalInterval,
    NominalName,
    NominalSupports,
    ParameterName,
    ParameterType,
    /// A `require` statement without its mandatory `else <value>` tail.
    RequireElse,
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
    /// A `throw` statement: the throw/catch channel was removed. A recoverable
    /// failure is a `Result<T, E>` value returned with `err(...)`.
    ThrowStatement,
    /// A block-form `try`/`catch`: removed in favour of prefix `try <expr>`
    /// propagation over a `Result<T, E>`.
    TryCatchBlock,
    /// A stray `catch` clause with no `try`: the throw/catch channel was removed.
    CatchClause,
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
