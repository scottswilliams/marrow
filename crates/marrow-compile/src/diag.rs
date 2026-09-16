//! Typed source diagnostics and the one bounded compiler diagnostic owner.
//!
//! A [`SourceDiagnostic`] couples a captured file identity with an opaque
//! payload: a syntax diagnostic retained whole, or a compiler-rendered finding.
//! Consumers read the frozen accessor set; construction is crate-private.
//!
//! Every production compiler diagnostic flows through the private
//! [`DiagnosticCollector`], which `finish` seals into a [`BoundedDiagnostics`]
//! terminal exactly once. Retention is bounded by the typed Count/OwnedBytes
//! ceilings — equal to the syntax collector's ceilings, drift-pinned below —
//! with count-before-bytes precedence. Crossing a ceiling discards the payload
//! destructively, prefix included, while the saturated totals keep
//! accumulating; an OwnedBytes limit may strengthen to Count, and a discarded
//! payload never re-materializes.

use std::fmt;

use crate::bounded::{Bounded, Ceiling};
use crate::decl::{DeclarationNamespace, RefusalReport};
use crate::source::ProjectFile;
use marrow_codes::Code;
use marrow_project::{FileIdentity, IdentityAnchor, IdentityKind, SourceOrigin};
use marrow_syntax::{
    Diagnostic, DiagnosticReason, Severity, SourceSpan, SyntaxDiagnosticLimit, SyntaxDiagnostics,
};

/// The most diagnostic rows the compiler collector retains before it discards
/// the payload and reports [`CompileDiagnosticLimit::Count`]. Equal to
/// [`marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT`]: the syntax bridge's Limited
/// composition law depends on that equality, which the drift test below pins.
pub(crate) const MAX_DIAGNOSTIC_COUNT: usize = 4096;

/// The most retained owned payload bytes the compiler collector holds before
/// it discards the payload and reports [`CompileDiagnosticLimit::OwnedBytes`].
/// Equal to [`marrow_syntax::SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT`]. The
/// budget is the logical initialized payload measured by
/// [`SourceDiagnostic::retained_owned_bytes`] — never a bound on `Vec` or
/// `String` allocation capacity.
pub(crate) const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

/// The one static invalid-UTF-8 message. It is `'static`, so an invalid-UTF-8
/// row charges its file spelling and zero message bytes.
const INVALID_UTF8_MESSAGE: &str = "source file is not valid UTF-8";

/// A non-UTF-8 file has no parsed construct to point at: a zero-length span at
/// the file start, whose 1-based point is 1:1.
const INVALID_UTF8_SPAN: SourceSpan = SourceSpan {
    start_byte: 0,
    end_byte: 0,
    line: 1,
    column: 1,
};

/// A single source diagnostic: the captured file it points into and its opaque
/// payload. A syntax finding is retained whole — code, reason, severity,
/// message, help, and span, never flattened — and a compiler finding carries its
/// rendered form, its typed identity gap, or the typed invalid-UTF-8 facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnostic {
    file: ProjectFile,
    payload: SourceDiagnosticPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SourceDiagnosticPayload {
    Syntax(Diagnostic),
    Compiler(CompilerDiagnostic),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompilerDiagnostic {
    /// An ordinary compiler finding: a stable code, the offending span, and the
    /// rendered message.
    Rendered {
        code: Code,
        span: SourceSpan,
        message: String,
    },
    /// A `check.durable_identity` finding carrying its typed gap, which the
    /// CLI's mint action consumes instead of the rendered message.
    IdentityGap {
        code: Code,
        span: SourceSpan,
        message: String,
        gap: IdentityGap,
    },
    /// A steer from a use site to the declaration this project wrote and the
    /// compiler refused, carrying the typed facts that tell it apart from a row
    /// about a name that was never declared.
    RefusedDeclaration {
        code: Code,
        span: SourceSpan,
        message: String,
        refused: RefusedDeclaration,
    },
    /// A finding whose message ends in a steer, carrying the typed target the
    /// steer names beside the rendered form.
    Steered {
        code: Code,
        span: SourceSpan,
        message: String,
        steer: Steer,
    },
    /// A file the drive could not decode. The message is the central static and
    /// the span the fixed file-start point, so this variant owns only the
    /// typed `Utf8Error` numbers.
    InvalidUtf8 {
        valid_up_to: usize,
        error_len: Option<usize>,
    },
}

/// Why a durable declaration's identity is incomplete: its ledger anchor has no
/// row (mintable), or it names a retired anchor that can never be reused.
///
/// The gap names the tree that *declares* the anchor, because that is the tree whose
/// `.marrow/ids` must gain the row. A gap owned by a dependency is not the consuming
/// project's to mint: the library commits its own ledger in its own directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityGap {
    pub kind: IdentityKind,
    pub path: String,
    pub retired: bool,
    pub origin: SourceOrigin,
}

impl IdentityGap {
    /// The `(kind, path)` anchor this gap names.
    pub fn anchor(&self) -> IdentityAnchor {
        IdentityAnchor::new(self.kind, self.path.clone())
    }
}

/// The typed facts of a steer to a refused declaration.
///
/// A steer and a row about a name that was never declared are both `check.*` rows at
/// the use span, and a steer reuses the *declaring* code rather than minting one of its
/// own, so `(code, line, column)` cannot tell them apart. A consumer reads these facts
/// instead: which ledger holds the refusal, the declaring diagnostic's code, and where
/// the report carrying the cause sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefusedDeclaration {
    /// The ledger holding the refusal. `None` only for a summary that has not been
    /// declared into one, a state no steer can reach.
    pub namespace: Option<DeclarationNamespace>,
    /// The stable code of the row reported at the declaration, which this steer
    /// reuses so the reader follows one code to one fix.
    pub declaring_code: Code,
    /// Where that report was made: at the declaration, by a covering pass, or by an
    /// earlier stage that refused the whole source.
    pub report: RefusalReport,
}

/// The family a name was looked up in, so a did-you-mean spells its candidate the way
/// that family is written: a store root reads back with its `^` sigil, a function or a
/// value reads back plainly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameFamily {
    Root,
    Function,
    Value,
}

/// Where a steering diagnostic sends the reader.
///
/// A steer reuses the code of the finding it rides — today every one of these is a
/// `check.type` — so the code and span cannot say what it names. These are the typed
/// facts: the candidate spelling a did-you-mean offers, and the branch a keyed-branch
/// steer points at. [`Display`](std::fmt::Display) is the one renderer; the prose lives
/// nowhere else, so payload and message cannot drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Steer {
    /// The nearest declared identifier to an unresolved name, offered as a single
    /// unambiguous candidate in its family's spelling.
    DidYouMean {
        family: NameFamily,
        candidate: String,
    },
    /// A keyed branch named where a field of a materialized entry record was expected.
    /// `resource` names the declaring resource when the bound value is a whole-entry
    /// record; a materialized *branch* entry value is not owned by the resource
    /// registry, so a sub-branch steer has none and renders generically. No store root
    /// is ever named: several roots may occur over one resource, so naming one would
    /// answer a declaration question with an occurrence.
    KeyedBranch {
        branch: String,
        resource: Option<String>,
    },
}

impl Steer {
    /// The owned payload bytes this steer charges against the diagnostic byte ceiling.
    fn retained_owned_bytes(&self) -> usize {
        match self {
            Self::DidYouMean { candidate, .. } => candidate.len(),
            Self::KeyedBranch { branch, resource } => {
                branch.len() + resource.as_ref().map_or(0, String::len)
            }
        }
    }
}

impl fmt::Display for Steer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DidYouMean {
                family: NameFamily::Root,
                candidate,
            } => write!(f, "Did you mean the store root `^{candidate}`?"),
            Self::DidYouMean {
                family: NameFamily::Function,
                candidate,
            } => write!(f, "Did you mean the function `{candidate}`?"),
            Self::DidYouMean {
                family: NameFamily::Value,
                candidate,
            } => write!(f, "Did you mean `{candidate}`?"),
            Self::KeyedBranch {
                branch,
                resource: Some(resource),
            } => write!(
                f,
                "`{branch}` is a keyed branch of `{resource}`, not a field of the bound entry \
                 value. A keyed branch is a distinct durable node reached through a store path, \
                 not projected from a materialized record. Read it directly with \
                 `^root[key].{branch}[branchKey]`, or bind the branch with a nested `if const`."
            ),
            Self::KeyedBranch {
                branch,
                resource: None,
            } => write!(
                f,
                "`{branch}` is a keyed branch, not a field of the bound entry value. A keyed \
                 branch is a distinct durable node reached through a store path, not projected \
                 from a materialized record. Read it through its durable path, or bind it with a \
                 nested `if const`."
            ),
        }
    }
}

impl SourceDiagnostic {
    pub(crate) fn at(code: Code, file: &ProjectFile, span: SourceSpan, message: String) -> Self {
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Compiler(CompilerDiagnostic::Rendered {
                code,
                span,
                message,
            }),
        }
    }

    pub(crate) fn with_identity_gap(
        code: Code,
        file: &ProjectFile,
        span: SourceSpan,
        message: String,
        gap: IdentityGap,
    ) -> Self {
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Compiler(CompilerDiagnostic::IdentityGap {
                code,
                span,
                message,
                gap,
            }),
        }
    }

    /// A steer to a refused declaration, carrying the typed facts beside its
    /// rendered form.
    ///
    /// `code` is the row's own code and `refused.declaring_code` the code of the report
    /// the reader is sent to. They match for every steer that reuses the declaring row,
    /// and differ for the identity class, whose cause is a report *family* the steer
    /// names under its own `check.type`.
    pub(crate) fn with_refused_declaration(
        code: Code,
        file: &ProjectFile,
        span: SourceSpan,
        message: String,
        refused: RefusedDeclaration,
    ) -> Self {
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Compiler(CompilerDiagnostic::RefusedDeclaration {
                code,
                span,
                message,
                refused,
            }),
        }
    }

    /// A finding whose message ends in a steer. `prefix` is the part of the message the
    /// steer does not own — empty when the steer is the whole message — and the steer
    /// renders the rest, so the typed payload and the prose are one construction.
    pub(crate) fn with_steer(
        code: Code,
        file: &ProjectFile,
        span: SourceSpan,
        prefix: &str,
        steer: Steer,
    ) -> Self {
        let message = if prefix.is_empty() {
            steer.to_string()
        } else {
            format!("{prefix}. {steer}")
        };
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Compiler(CompilerDiagnostic::Steered {
                code,
                span,
                message,
                steer,
            }),
        }
    }

    pub(crate) fn invalid_utf8(
        file: &ProjectFile,
        valid_up_to: usize,
        error_len: Option<usize>,
    ) -> Self {
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 {
                valid_up_to,
                error_len,
            }),
        }
    }

    /// A syntax row absorbed by the bridge: the original diagnostic retained
    /// whole under the file it was parsed from.
    fn syntax(file: &ProjectFile, diagnostic: Diagnostic) -> Self {
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Syntax(diagnostic),
        }
    }

    /// The typed identity of this diagnostic. A renderer spells it with
    /// [`Code::as_str`] at the boundary; nothing compares the spelling.
    pub fn code(&self) -> Code {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => diagnostic.typed_code(),
            SourceDiagnosticPayload::Compiler(
                CompilerDiagnostic::Rendered { code, .. }
                | CompilerDiagnostic::IdentityGap { code, .. }
                | CompilerDiagnostic::RefusedDeclaration { code, .. }
                | CompilerDiagnostic::Steered { code, .. },
            ) => *code,
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 { .. }) => {
                Code::CheckUnsupported
            }
        }
    }

    /// The rendered message.
    pub fn message(&self) -> &str {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => &diagnostic.message,
            SourceDiagnosticPayload::Compiler(
                CompilerDiagnostic::Rendered { message, .. }
                | CompilerDiagnostic::IdentityGap { message, .. }
                | CompilerDiagnostic::RefusedDeclaration { message, .. }
                | CompilerDiagnostic::Steered { message, .. },
            ) => message,
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 { .. }) => {
                INVALID_UTF8_MESSAGE
            }
        }
    }

    /// The help text of a syntax finding; a compiler finding carries none.
    pub fn help(&self) -> Option<&str> {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => diagnostic.help.as_deref(),
            SourceDiagnosticPayload::Compiler(_) => None,
        }
    }

    /// The severity. Every syntax producer constructs `Error`, and every
    /// compiler finding is an error, so the whole surface is error-severity.
    pub fn severity(&self) -> Severity {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => diagnostic.severity,
            SourceDiagnosticPayload::Compiler(_) => Severity::Error,
        }
    }

    /// The typed reason of a syntax finding; a compiler finding carries none.
    pub fn reason(&self) -> Option<&DiagnosticReason> {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => Some(&diagnostic.reason),
            SourceDiagnosticPayload::Compiler(_) => None,
        }
    }

    /// The typed durable-identity gap behind a `check.durable_identity`
    /// diagnostic, `None` for every other payload. The CLI's `marrow run` mint
    /// action consumes this — never the rendered message — to learn which
    /// anchors to mint, so the classifier stays in the compiler.
    pub fn identity_gap(&self) -> Option<&IdentityGap> {
        match &self.payload {
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::IdentityGap { gap, .. }) => {
                Some(gap)
            }
            _ => None,
        }
    }

    /// The typed facts behind a steer to a refused declaration, `None` for every
    /// other payload.
    ///
    /// A causality assertion reads these instead of the rendered prose, which is not a
    /// contract: a steer and a row about a genuinely absent name are
    /// `(code, line, column)`-identical.
    pub fn refused_declaration(&self) -> Option<&RefusedDeclaration> {
        match &self.payload {
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::RefusedDeclaration {
                refused,
                ..
            }) => Some(refused),
            _ => None,
        }
    }

    /// The typed target of a steering diagnostic, `None` for every other payload.
    ///
    /// A steer rides the code of the finding it corrects, so `(code, line, column)` says
    /// only that something is ill-typed. A test that means to pin the steer reads this
    /// instead of the rendered prose, which is not a contract.
    pub fn steer(&self) -> Option<&Steer> {
        match &self.payload {
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::Steered { steer, .. }) => {
                Some(steer)
            }
            _ => None,
        }
    }

    /// The captured source file this diagnostic points into. Always a canonical
    /// bounded identity — never empty, a sentinel, or a consumer-chosen placeholder.
    /// It is relative to the root of [`Self::origin`]'s tree, so a renderer that
    /// rejoins it to a directory must use that tree's root.
    pub fn file(&self) -> &FileIdentity {
        self.file.identity()
    }

    /// The tree this diagnostic's file was captured from: the root project, or the
    /// dependency the root declares under an alias.
    pub fn origin(&self) -> &SourceOrigin {
        self.file.origin()
    }

    /// The full UTF-8 span of the offending construct.
    pub fn span(&self) -> SourceSpan {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => diagnostic.span,
            SourceDiagnosticPayload::Compiler(
                CompilerDiagnostic::Rendered { span, .. }
                | CompilerDiagnostic::IdentityGap { span, .. }
                | CompilerDiagnostic::RefusedDeclaration { span, .. }
                | CompilerDiagnostic::Steered { span, .. },
            ) => *span,
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 { .. }) => {
                INVALID_UTF8_SPAN
            }
        }
    }

    /// The 1-based start line of the diagnostic, read from its span.
    pub fn line(&self) -> u32 {
        self.span().line
    }

    /// The 1-based start column of the diagnostic, read from its span.
    pub fn column(&self) -> u32 {
        self.span().column
    }

    /// The typed facts of an invalid-UTF-8 row: how many leading bytes decoded
    /// and the invalid sequence length `std::str::from_utf8` reported. This probe
    /// pins them in tests until a production reader exists.
    #[cfg(test)]
    pub(crate) fn invalid_utf8_facts(&self) -> Option<(usize, Option<usize>)> {
        match &self.payload {
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 {
                valid_up_to,
                error_len,
            }) => Some((*valid_up_to, *error_len)),
            _ => None,
        }
    }

    /// The retained variable payload bytes this row charges against
    /// [`MAX_DIAGNOSTIC_BYTES`]: its file address — the spelling plus the short
    /// declaring alias — plus its owned message, syntax help, and identity-gap
    /// path bytes. A logical initialized-payload
    /// budget — never `Vec`/`String` capacity or allocator metadata. Static
    /// facts (the invalid-UTF-8 message, codes, spans) charge nothing. Syntax
    /// reason-owned bytes are charged at the `absorb_syntax` boundary instead,
    /// so this per-row charge and the batch charge agree.
    pub(crate) fn retained_owned_bytes(&self) -> usize {
        let file = self.file.retained_owned_bytes();
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => {
                file + diagnostic.message.len() + diagnostic.help.as_deref().map_or(0, str::len)
            }
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::Rendered { message, .. }) => {
                file + message.len()
            }
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::IdentityGap {
                message,
                gap,
                ..
            }) => {
                file + message.len()
                    + gap.path.len()
                    + gap.origin.alias().map_or(0, |alias| alias.as_str().len())
            }
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::RefusedDeclaration {
                message,
                ..
            }) => file + message.len(),
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::Steered {
                message,
                steer,
                ..
            }) => file + message.len() + steer.retained_owned_bytes(),
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 { .. }) => file,
        }
    }
}

/// The typed compiler ceiling that discarded a diagnostic payload. Maps
/// exhaustively to `ResourceLimitKind::{DiagnosticCount, DiagnosticBytes}` at
/// the failure boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompileDiagnosticLimit {
    Count { limit: usize },
    OwnedBytes { limit: usize },
}

/// The one live compiler diagnostic owner. Private, concrete, non-`Clone`, and
/// non-`Default`, so a collector can only be moved whole.
#[derive(Debug)]
pub(crate) struct DiagnosticCollector {
    state: Bounded<DiagnosticCeiling>,
}

/// The diagnostic ceilings, as the shared bounded owner reads them.
struct DiagnosticCeiling;

impl Ceiling for DiagnosticCeiling {
    type Payload = Vec<SourceDiagnostic>;
    type Limit = CompileDiagnosticLimit;

    const MAX_COUNT: u64 = MAX_DIAGNOSTIC_COUNT as u64;
    const MAX_BYTES: u64 = MAX_DIAGNOSTIC_BYTES as u64;

    fn count_limit() -> Self::Limit {
        CompileDiagnosticLimit::Count {
            limit: MAX_DIAGNOSTIC_COUNT,
        }
    }

    fn bytes_limit() -> Self::Limit {
        CompileDiagnosticLimit::OwnedBytes {
            limit: MAX_DIAGNOSTIC_BYTES,
        }
    }

    fn is_bytes(limit: Self::Limit) -> bool {
        matches!(limit, CompileDiagnosticLimit::OwnedBytes { .. })
    }
}

/// The finished terminal of one collector: the complete ordered payload with
/// its exact byte total, or the typed limit with saturated totals and no
/// payload.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoundedDiagnostics {
    Complete {
        owned_bytes: usize,
        rows: Vec<SourceDiagnostic>,
    },
    Limited {
        count: usize,
        owned_bytes: usize,
        limit: CompileDiagnosticLimit,
    },
}

impl BoundedDiagnostics {
    /// Logical emptiness: a Limited terminal retains no rows but is never
    /// empty — its limit displaced at least one row.
    pub(crate) fn is_empty(&self) -> bool {
        matches!(self, BoundedDiagnostics::Complete { rows, .. } if rows.is_empty())
    }

    /// Test support: the complete rows, or a panic on a limited terminal.
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn expect_complete(self) -> Vec<SourceDiagnostic> {
        match self {
            BoundedDiagnostics::Complete { rows, .. } => rows,
            BoundedDiagnostics::Limited { limit, .. } => {
                panic!("expected a complete terminal, got {limit:?}")
            }
        }
    }
}

/// A test view of a collector's exact state, so lifecycle probes can compare
/// owners without widening the production operation set.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectorProbe {
    pub(crate) count: usize,
    pub(crate) owned_bytes: usize,
    pub(crate) limit: Option<CompileDiagnosticLimit>,
    pub(crate) rows: Vec<SourceDiagnostic>,
}

impl DiagnosticCollector {
    pub(crate) fn new() -> Self {
        Self {
            state: Bounded::new(),
        }
    }

    /// Logical emptiness: a Limited owner retains no rows but is never empty.
    /// Test-only — production code reads emptiness from the finished
    /// [`BoundedDiagnostics`] terminal, never from a live collector.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.state.is_empty()
    }

    /// Retain one finalized row, charging its exact retained owned bytes.
    pub(crate) fn push(&mut self, row: SourceDiagnostic) {
        let bytes = row.retained_owned_bytes() as u64;
        self.state
            .admit((0, 0), 1, bytes, move |rows| rows.push(row));
    }

    /// Merge a finished terminal into this live owner. A Complete terminal's
    /// rows append in order with their exact totals; a Limited terminal leaves
    /// this owner Limited unconditionally, its payload gone for good.
    pub(crate) fn absorb(&mut self, finished: BoundedDiagnostics) {
        match finished {
            BoundedDiagnostics::Complete {
                owned_bytes,
                mut rows,
            } => self.state.admit(
                (0, 0),
                rows.len() as u64,
                owned_bytes as u64,
                move |retained| retained.append(&mut rows),
            ),
            BoundedDiagnostics::Limited {
                count,
                owned_bytes,
                limit,
            } => self
                .state
                .absorb_limited(count as u64, owned_bytes as u64, limit),
        }
    }

    /// The sole syntax bridge: consume one parsed file's bounded terminal.
    /// The batch charge is the summary's owned bytes plus one file address per
    /// row, which is exactly the per-row
    /// [`SourceDiagnostic::retained_owned_bytes`] sum. A Limited terminal
    /// composes the same charge from its saturated summary and leaves this
    /// owner Limited unconditionally, selecting Count when both composed kinds
    /// have crossed.
    pub(crate) fn absorb_syntax(&mut self, file: &ProjectFile, diagnostics: SyntaxDiagnostics) {
        let summary = diagnostics.summary();
        let charge = |count: usize| {
            summary
                .owned_bytes()
                .saturating_add(count.saturating_mul(file.retained_owned_bytes()))
                as u64
        };
        match diagnostics.into_complete() {
            Ok(payload) => {
                let mut rows: Vec<SourceDiagnostic> = payload
                    .into_boxed_slice()
                    .into_iter()
                    .map(|diagnostic| SourceDiagnostic::syntax(file, diagnostic))
                    .collect();
                // The materialized vector is the one quantity both charges derive from.
                let count = rows.len();
                self.state
                    .admit((0, 0), count as u64, charge(count), move |retained| {
                        retained.append(&mut rows);
                    });
            }
            Err(limit) => {
                let inherited = match limit {
                    SyntaxDiagnosticLimit::Count { .. } => DiagnosticCeiling::count_limit(),
                    SyntaxDiagnosticLimit::OwnedBytes { .. } => DiagnosticCeiling::bytes_limit(),
                };
                // A Limited terminal destroyed its payload, so its saturated summary
                // count is the only quantity left to charge.
                let count = summary.count();
                self.state
                    .absorb_limited(count as u64, charge(count), inherited);
            }
        }
    }

    /// Seal this owner into its terminal. Total: every state has a terminal.
    pub(crate) fn finish(self) -> BoundedDiagnostics {
        match self.state {
            Bounded::Retaining { bytes, payload, .. } => BoundedDiagnostics::Complete {
                owned_bytes: bytes as usize,
                rows: payload,
            },
            Bounded::Limited {
                count,
                bytes,
                limit,
            } => BoundedDiagnostics::Limited {
                count: count as usize,
                owned_bytes: bytes as usize,
                limit,
            },
        }
    }

    /// Test view of the exact owner state.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> CollectorProbe {
        let (count, owned_bytes) = self.state.totals();
        CollectorProbe {
            count: count as usize,
            owned_bytes: owned_bytes as usize,
            limit: self.state.limit(),
            rows: self.probe_rows().to_vec(),
        }
    }

    /// Test view of the retained rows (empty once Limited).
    #[cfg(test)]
    pub(crate) fn probe_rows(&self) -> &[SourceDiagnostic] {
        match &self.state {
            Bounded::Retaining { payload, .. } => payload,
            Bounded::Limited { .. } => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marrow_syntax::{SYNTAX_DIAGNOSTIC_COUNT_LIMIT, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT};

    /// The syntax bridge's Limited composition depends on the two ceiling pairs
    /// being equal; a divergence is a contract change that must update both.
    #[test]
    fn compiler_ceilings_equal_the_syntax_ceilings() {
        assert_eq!(MAX_DIAGNOSTIC_COUNT, SYNTAX_DIAGNOSTIC_COUNT_LIMIT);
        assert_eq!(MAX_DIAGNOSTIC_BYTES, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT);
    }

    fn file() -> &'static ProjectFile {
        crate::test_file("src/main.mw")
    }

    /// A rendered compiler row whose retained owned bytes are exactly
    /// `file.len() + message_len`.
    fn row_with_message_len(message_len: usize) -> SourceDiagnostic {
        SourceDiagnostic::at(
            Code::CheckType,
            file(),
            SourceSpan::default(),
            "x".repeat(message_len),
        )
    }

    /// The byte-charge law, per payload variant: file spelling plus owned
    /// message, syntax help, identity-gap path, and steer-payload bytes; the
    /// static invalid-UTF-8 message charges nothing beyond the file.
    #[test]
    fn retained_owned_bytes_charges_each_payload_component_exactly() {
        let file_len = file().retained_owned_bytes();

        let rendered = row_with_message_len(10);
        assert_eq!(rendered.retained_owned_bytes(), file_len + 10);

        let gap = SourceDiagnostic::with_identity_gap(
            Code::CheckDurableIdentity,
            file(),
            SourceSpan::default(),
            "y".repeat(7),
            IdentityGap {
                kind: IdentityKind::Root,
                path: "^books".to_string(),
                retired: false,
                origin: SourceOrigin::Root,
            },
        );
        assert_eq!(gap.retained_owned_bytes(), file_len + 7 + "^books".len());

        let steered = SourceDiagnostic::with_steer(
            Code::CheckType,
            file(),
            SourceSpan::default(),
            "`membrs` is not in scope",
            Steer::DidYouMean {
                family: NameFamily::Root,
                candidate: "members".to_string(),
            },
        );
        assert_eq!(
            steered.message(),
            "`membrs` is not in scope. Did you mean the store root `^members`?",
        );
        assert_eq!(
            steered.retained_owned_bytes(),
            file_len + steered.message().len() + "members".len()
        );

        let utf8 = SourceDiagnostic::invalid_utf8(file(), 3, Some(1));
        assert_eq!(utf8.retained_owned_bytes(), file_len);
        assert_eq!(utf8.invalid_utf8_facts(), Some((3, Some(1))));
        assert_eq!(utf8.message(), "source file is not valid UTF-8");
        assert_eq!(utf8.code(), Code::CheckUnsupported);
        let span = utf8.span();
        assert_eq!(
            (span.start_byte, span.end_byte, span.line, span.column),
            (0, 0, 1, 1)
        );

        let parsed = marrow_syntax::parse_source("@\n");
        let complete = parsed
            .diagnostics
            .as_complete()
            .expect("one lexer row is complete")
            .as_slice()
            .to_vec();
        let syntax = SourceDiagnostic::syntax(file(), complete[0].clone());
        assert_eq!(
            syntax.retained_owned_bytes(),
            file_len + complete[0].message.len() + complete[0].help.as_deref().map_or(0, str::len)
        );
        assert!(syntax.reason().is_some());
        assert_eq!(syntax.severity(), Severity::Error);
    }

    /// Exact count edge: the row at the ceiling retains; the next destroys the
    /// whole payload, prefix included, and the owner stays non-empty after.
    #[test]
    fn count_edge_is_exact_and_discard_is_destructive() {
        let mut collector = DiagnosticCollector::new();
        for _ in 0..MAX_DIAGNOSTIC_COUNT {
            collector.push(row_with_message_len(1));
        }
        let at_edge = collector.probe();
        assert_eq!(at_edge.count, MAX_DIAGNOSTIC_COUNT);
        assert_eq!(at_edge.limit, None);
        assert_eq!(at_edge.rows.len(), MAX_DIAGNOSTIC_COUNT);

        collector.push(row_with_message_len(1));
        let crossed = collector.probe();
        assert_eq!(crossed.count, MAX_DIAGNOSTIC_COUNT + 1);
        assert_eq!(
            crossed.limit,
            Some(CompileDiagnosticLimit::Count {
                limit: MAX_DIAGNOSTIC_COUNT
            })
        );
        assert!(crossed.rows.is_empty(), "the prefix drops with the payload");
        assert!(!collector.is_empty(), "a limited owner is never empty");

        // Totals keep saturating; the payload never re-materializes.
        collector.push(row_with_message_len(1));
        assert_eq!(collector.probe().count, MAX_DIAGNOSTIC_COUNT + 1);
        assert!(collector.probe_rows().is_empty());
        assert!(matches!(
            collector.finish(),
            BoundedDiagnostics::Limited { .. }
        ));
    }

    /// Exact byte edges: a payload of exactly the ceiling retains; one more
    /// byte destroys it for an OwnedBytes limit with saturated byte totals.
    #[test]
    fn byte_edge_is_exact_and_saturates_at_ceiling_plus_one() {
        let file_len = file().retained_owned_bytes();
        let first = MAX_DIAGNOSTIC_BYTES / 2;
        let second = MAX_DIAGNOSTIC_BYTES - first - 2 * file_len;

        let mut collector = DiagnosticCollector::new();
        collector.push(row_with_message_len(first));
        collector.push(row_with_message_len(second));
        let at_edge = collector.probe();
        assert_eq!(at_edge.owned_bytes, MAX_DIAGNOSTIC_BYTES);
        assert_eq!(at_edge.limit, None);
        assert_eq!(at_edge.rows.len(), 2);

        let mut crossing = DiagnosticCollector::new();
        crossing.push(row_with_message_len(first));
        crossing.push(row_with_message_len(second + 1));
        let crossed = crossing.probe();
        assert_eq!(crossed.owned_bytes, MAX_DIAGNOSTIC_BYTES + 1);
        assert_eq!(
            crossed.limit,
            Some(CompileDiagnosticLimit::OwnedBytes {
                limit: MAX_DIAGNOSTIC_BYTES
            })
        );
        assert!(crossed.rows.is_empty());

        // Later input saturates the byte total at ceiling plus one.
        crossing.push(row_with_message_len(100));
        assert_eq!(crossing.probe().owned_bytes, MAX_DIAGNOSTIC_BYTES + 1);
    }

    /// Count wins a simultaneous crossing, and an OwnedBytes limit
    /// strengthens to Count once the composed count crosses — never the
    /// reverse.
    #[test]
    fn count_precedence_and_bytes_to_count_strengthening() {
        // One admission crossing both ceilings at once: Count is selected.
        let mut both = DiagnosticCollector::new();
        for _ in 0..MAX_DIAGNOSTIC_COUNT {
            both.push(row_with_message_len(1));
        }
        both.push(row_with_message_len(MAX_DIAGNOSTIC_BYTES));
        assert_eq!(
            both.probe().limit,
            Some(CompileDiagnosticLimit::Count {
                limit: MAX_DIAGNOSTIC_COUNT
            })
        );

        // Bytes first, then the count total crosses: strengthened to Count.
        let mut strengthened = DiagnosticCollector::new();
        strengthened.push(row_with_message_len(MAX_DIAGNOSTIC_BYTES + 1));
        assert_eq!(
            strengthened.probe().limit,
            Some(CompileDiagnosticLimit::OwnedBytes {
                limit: MAX_DIAGNOSTIC_BYTES
            })
        );
        for _ in 0..MAX_DIAGNOSTIC_COUNT {
            strengthened.push(row_with_message_len(0));
        }
        assert_eq!(
            strengthened.probe().limit,
            Some(CompileDiagnosticLimit::Count {
                limit: MAX_DIAGNOSTIC_COUNT
            })
        );
        assert!(strengthened.probe_rows().is_empty());
    }

    /// `absorb` merges a Complete terminal in order with its exact totals; a
    /// Limited terminal forces this owner Limited unconditionally and selects
    /// Count when the composed count has crossed.
    #[test]
    fn absorb_merges_complete_terminals_and_forces_limited_ones() {
        let mut source = DiagnosticCollector::new();
        source.push(row_with_message_len(3));
        source.push(row_with_message_len(5));
        let terminal = source.finish();

        let mut target = DiagnosticCollector::new();
        target.push(row_with_message_len(1));
        target.absorb(terminal);
        let merged = target.probe();
        assert_eq!(merged.count, 3);
        assert_eq!(
            merged.owned_bytes,
            3 * file().retained_owned_bytes() + 1 + 3 + 5
        );
        assert_eq!(
            merged.rows[1].retained_owned_bytes(),
            file().retained_owned_bytes() + 3
        );

        // With the composed count crossed, Count outranks the absorbed kind.
        let mut nearly_full = DiagnosticCollector::new();
        for _ in 0..MAX_DIAGNOSTIC_COUNT {
            nearly_full.push(row_with_message_len(0));
        }
        nearly_full.absorb(BoundedDiagnostics::Limited {
            count: 2,
            owned_bytes: MAX_DIAGNOSTIC_BYTES + 1,
            limit: CompileDiagnosticLimit::OwnedBytes {
                limit: MAX_DIAGNOSTIC_BYTES,
            },
        });
        assert_eq!(
            nearly_full.probe().limit,
            Some(CompileDiagnosticLimit::Count {
                limit: MAX_DIAGNOSTIC_COUNT
            })
        );
        assert!(nearly_full.probe_rows().is_empty());
    }

    /// Absorbing a Limited terminal whose composed totals sit under *both*
    /// ceilings still leaves this owner Limited, with the absorbed kind
    /// inherited: the absorbed payload was destroyed, so admissible-looking
    /// totals must never reopen the owner and seal a silently short set.
    #[test]
    fn absorbing_an_under_ceiling_limited_terminal_forces_limited() {
        for inherited in [
            CompileDiagnosticLimit::Count {
                limit: MAX_DIAGNOSTIC_COUNT,
            },
            CompileDiagnosticLimit::OwnedBytes {
                limit: MAX_DIAGNOSTIC_BYTES,
            },
        ] {
            let mut empty = DiagnosticCollector::new();
            empty.absorb(BoundedDiagnostics::Limited {
                count: 1,
                owned_bytes: 1,
                limit: inherited,
            });
            let probe = empty.probe();
            assert_eq!(
                probe.limit,
                Some(inherited),
                "an under-ceiling Limited terminal still forces Limited and keeps its kind"
            );
            assert!(probe.rows.is_empty());
            assert!(!empty.is_empty(), "a limited owner is never empty");
            empty.push(row_with_message_len(1));
            assert!(
                empty.probe_rows().is_empty(),
                "the destroyed payload never re-materializes"
            );
            assert!(
                matches!(empty.finish(), BoundedDiagnostics::Limited { .. }),
                "an absorbed Limited terminal is never sealed Complete"
            );

            // The same absorption over a retaining prefix: the prefix drops too.
            let mut retaining = DiagnosticCollector::new();
            retaining.push(row_with_message_len(1));
            retaining.absorb(BoundedDiagnostics::Limited {
                count: 1,
                owned_bytes: 1,
                limit: inherited,
            });
            assert_eq!(retaining.probe().limit, Some(inherited));
            assert!(
                retaining.probe_rows().is_empty(),
                "the retained prefix drops with the absorbed payload"
            );
            assert!(
                matches!(retaining.finish(), BoundedDiagnostics::Limited { .. }),
                "an absorbed Limited terminal is never sealed Complete"
            );
        }
    }

    /// Why the state above is unreachable from production: a sealed Limited
    /// terminal always reports at least one total past the ceiling it names, on
    /// both sides of the bridge, so while the two ceiling pairs are equal every
    /// composed absorption crosses again on its own. That equality is the
    /// premise; the unconditional guard above holds if it ever changes.
    #[test]
    fn a_sealed_limited_terminal_always_reports_a_crossed_total() {
        let mut counted = DiagnosticCollector::new();
        for _ in 0..=MAX_DIAGNOSTIC_COUNT {
            counted.push(row_with_message_len(1));
        }
        let BoundedDiagnostics::Limited { count, .. } = counted.finish() else {
            panic!("crossing the count ceiling seals Limited");
        };
        assert!(count > MAX_DIAGNOSTIC_COUNT);

        let mut sized = DiagnosticCollector::new();
        sized.push(row_with_message_len(MAX_DIAGNOSTIC_BYTES + 1));
        let BoundedDiagnostics::Limited { owned_bytes, .. } = sized.finish() else {
            panic!("crossing the byte ceiling seals Limited");
        };
        assert!(owned_bytes > MAX_DIAGNOSTIC_BYTES);

        let dense = "@\n".repeat(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
        let summary = marrow_syntax::parse_source(&dense).diagnostics.summary();
        assert!(
            summary.count() > MAX_DIAGNOSTIC_COUNT || summary.owned_bytes() > MAX_DIAGNOSTIC_BYTES,
            "a limited syntax summary crosses a compiler ceiling on its own"
        );
    }

    /// The syntax bridge charges the summary's owned bytes plus one file
    /// address per row — so a longer file identity crosses the byte ceiling
    /// where a shorter one retains, at the exact edge.
    #[test]
    fn absorb_syntax_multiplies_the_file_spelling_by_the_row_count() {
        let source = "@\n@\n@\n@\n";
        let summary = marrow_syntax::parse_source(source).diagnostics.summary();
        let short = crate::test_file("src/a.mw");
        let long = crate::test_file("src/abcdefgh.mw");
        assert_eq!(
            long.retained_owned_bytes(),
            short.retained_owned_bytes() + 7
        );

        // Fill so that absorbing under the short path lands exactly at the
        // ceiling and under the long path crosses it.
        let batch = |identity: &ProjectFile| {
            summary.owned_bytes() + summary.count() * identity.retained_owned_bytes()
        };
        let prefill = MAX_DIAGNOSTIC_BYTES - batch(short) - file().retained_owned_bytes();

        let mut exact = DiagnosticCollector::new();
        exact.push(row_with_message_len(prefill));
        exact.absorb_syntax(short, marrow_syntax::parse_source(source).diagnostics);
        let at_edge = exact.probe();
        assert_eq!(at_edge.owned_bytes, MAX_DIAGNOSTIC_BYTES);
        assert_eq!(at_edge.limit, None);
        assert_eq!(at_edge.count, 1 + summary.count());

        let mut crossing = DiagnosticCollector::new();
        crossing.push(row_with_message_len(prefill));
        crossing.absorb_syntax(long, marrow_syntax::parse_source(source).diagnostics);
        assert_eq!(
            crossing.probe().limit,
            Some(CompileDiagnosticLimit::OwnedBytes {
                limit: MAX_DIAGNOSTIC_BYTES
            })
        );
    }

    /// The bridge's batch charge equals the per-row retained-owned-bytes sum,
    /// and absorbed rows keep their file identity and position order.
    #[test]
    fn absorb_syntax_batch_charge_equals_the_per_row_sum() {
        let parsed = marrow_syntax::parse_source("@\n@\n");
        let summary = parsed.diagnostics.summary();
        let mut collector = DiagnosticCollector::new();
        collector.absorb_syntax(file(), parsed.diagnostics);
        let probe = collector.probe();
        assert_eq!(probe.count, 2);
        assert_eq!(
            probe.owned_bytes,
            summary.owned_bytes() + 2 * file().retained_owned_bytes()
        );
        assert_eq!(
            probe.owned_bytes,
            probe
                .rows
                .iter()
                .map(SourceDiagnostic::retained_owned_bytes)
                .sum::<usize>()
        );
        assert_eq!(
            probe
                .rows
                .iter()
                .map(SourceDiagnostic::line)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(probe.rows.iter().all(|row| row.file() == file().identity()));
    }

    /// Absorbing a Limited syntax terminal leaves the collector Limited even
    /// though it retained nothing itself, and the destroyed payload never
    /// re-materializes through later input.
    #[test]
    fn absorbing_a_limited_syntax_terminal_forces_limited_unconditionally() {
        let dense = "@\n".repeat(SYNTAX_DIAGNOSTIC_COUNT_LIMIT + 1);
        let parsed = marrow_syntax::parse_source(&dense);
        assert!(parsed.diagnostics.as_complete().is_err());

        let mut collector = DiagnosticCollector::new();
        collector.absorb_syntax(file(), parsed.diagnostics);
        let probe = collector.probe();
        assert_eq!(
            probe.limit,
            Some(CompileDiagnosticLimit::Count {
                limit: MAX_DIAGNOSTIC_COUNT
            })
        );
        assert!(probe.rows.is_empty());
        assert!(!collector.is_empty());

        collector.push(row_with_message_len(1));
        assert!(collector.probe_rows().is_empty());
        assert!(matches!(
            collector.finish(),
            BoundedDiagnostics::Limited { .. }
        ));
    }

    /// Logical emptiness across states and terminals.
    #[test]
    fn is_empty_is_logical_not_representational() {
        let mut collector = DiagnosticCollector::new();
        assert!(collector.is_empty());
        collector.push(row_with_message_len(1));
        assert!(!collector.is_empty());

        let empty_terminal = DiagnosticCollector::new().finish();
        assert!(empty_terminal.is_empty());
        assert!(
            !BoundedDiagnostics::Limited {
                count: MAX_DIAGNOSTIC_COUNT + 1,
                owned_bytes: 0,
                limit: CompileDiagnosticLimit::Count {
                    limit: MAX_DIAGNOSTIC_COUNT
                },
            }
            .is_empty()
        );
    }
}
