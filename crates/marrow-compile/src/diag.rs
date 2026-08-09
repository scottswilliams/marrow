//! Typed source diagnostics and the one bounded compiler diagnostic owner.
//!
//! A [`SourceDiagnostic`] couples a captured file identity with an opaque
//! payload: a syntax diagnostic retained whole, or a compiler-rendered finding.
//! Consumers read the frozen accessor set; construction is crate-private.
//!
//! Every production compiler diagnostic flows through the private
//! [`DiagnosticCollector`]: producers `push` rows, the drive's syntax bridge
//! `absorb_syntax`es one parsed file's terminal at a time, finished terminals
//! merge through `absorb`, and `finish` seals the owner into a
//! [`BoundedDiagnostics`] terminal exactly once. Retention is bounded by the
//! typed Count/OwnedBytes ceilings — equal to the syntax collector's ceilings,
//! drift-pinned below — with count-before-bytes precedence; crossing a ceiling
//! discards the payload destructively (prefix included) while the saturated
//! totals keep accumulating, an OwnedBytes limit may strengthen to Count, and
//! a discarded payload never re-materializes.

use marrow_codes::Code;
use marrow_project::{FileIdentity, IdentityAnchor, IdentityKind};
use marrow_syntax::{
    Diagnostic, DiagnosticReason, Severity, SourceSpan, SyntaxDiagnosticLimit, SyntaxDiagnostics,
};

/// The most diagnostic rows the compiler collector retains before it discards
/// the payload and reports [`CompileDiagnosticLimit::Count`]. Equal to
/// [`marrow_syntax::SYNTAX_DIAGNOSTIC_COUNT_LIMIT`] (A7): the syntax bridge's
/// Limited composition law depends on the equality, which the drift test below
/// pins. Any later divergence is an observable contract change.
pub(crate) const MAX_DIAGNOSTIC_COUNT: usize = 4096;

/// The most retained owned payload bytes the compiler collector holds before
/// it discards the payload and reports [`CompileDiagnosticLimit::OwnedBytes`].
/// Equal to [`marrow_syntax::SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT`] (A7). The
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
/// payload. A syntax finding is retained whole — original code, reason,
/// severity, message, help, and span, never flattened — and a compiler finding
/// carries its rendered form, its typed identity gap, or the typed
/// invalid-UTF-8 facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDiagnostic {
    file: FileIdentity,
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
        code: &'static str,
        span: SourceSpan,
        message: String,
    },
    /// A `check.durable_identity` finding carrying its typed gap, which the
    /// CLI's mint action consumes instead of the rendered message.
    IdentityGap {
        code: &'static str,
        span: SourceSpan,
        message: String,
        gap: IdentityGap,
    },
    /// A file the drive could not decode: the typed `Utf8Error` facts. The
    /// message is the central static and the span is the fixed file-start
    /// point, so this variant owns only the numeric facts.
    InvalidUtf8 {
        valid_up_to: usize,
        error_len: Option<usize>,
    },
}

/// Why a durable declaration's identity is incomplete: its ledger anchor has no
/// row (mintable), or it names a retired anchor that can never be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityGap {
    pub kind: IdentityKind,
    pub path: String,
    pub retired: bool,
}

impl IdentityGap {
    /// The `(kind, path)` anchor this gap names.
    pub fn anchor(&self) -> IdentityAnchor {
        IdentityAnchor::new(self.kind, self.path.clone())
    }
}

impl SourceDiagnostic {
    pub(crate) fn at(
        code: &'static str,
        file: &FileIdentity,
        span: SourceSpan,
        message: String,
    ) -> Self {
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
        code: &'static str,
        file: &FileIdentity,
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

    pub(crate) fn invalid_utf8(
        file: &FileIdentity,
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
    fn syntax(file: &FileIdentity, diagnostic: Diagnostic) -> Self {
        Self {
            file: file.clone(),
            payload: SourceDiagnosticPayload::Syntax(diagnostic),
        }
    }

    /// The stable `marrow-codes` string of this diagnostic.
    pub fn code(&self) -> &'static str {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => diagnostic.code,
            SourceDiagnosticPayload::Compiler(
                CompilerDiagnostic::Rendered { code, .. }
                | CompilerDiagnostic::IdentityGap { code, .. },
            ) => code,
            SourceDiagnosticPayload::Compiler(CompilerDiagnostic::InvalidUtf8 { .. }) => {
                Code::CheckUnsupported.as_str()
            }
        }
    }

    /// The rendered message.
    pub fn message(&self) -> &str {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => &diagnostic.message,
            SourceDiagnosticPayload::Compiler(
                CompilerDiagnostic::Rendered { message, .. }
                | CompilerDiagnostic::IdentityGap { message, .. },
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

    /// The captured source file this diagnostic points into. Always a canonical
    /// FIDB01-bounded identity — never empty, a sentinel, or a consumer-chosen
    /// placeholder.
    pub fn file(&self) -> &FileIdentity {
        &self.file
    }

    /// The full UTF-8 span of the offending construct.
    pub fn span(&self) -> SourceSpan {
        match &self.payload {
            SourceDiagnosticPayload::Syntax(diagnostic) => diagnostic.span,
            SourceDiagnosticPayload::Compiler(
                CompilerDiagnostic::Rendered { span, .. }
                | CompilerDiagnostic::IdentityGap { span, .. },
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
    /// and the invalid sequence length `std::str::from_utf8` reported. The
    /// facts live in the payload for every consumer of the variant; this
    /// probe pins them in tests until a production reader exists.
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
    /// [`MAX_DIAGNOSTIC_BYTES`]: its file spelling plus its owned message,
    /// syntax help, and identity-gap path bytes. A logical initialized-payload
    /// budget — never `Vec`/`String` capacity or allocator metadata. Static
    /// facts (the invalid-UTF-8 message, codes, spans) charge nothing. Syntax
    /// reason-owned bytes are charged at the `absorb_syntax` boundary from the
    /// syntax summary (every current reason variant owns zero), so this per-row
    /// charge and the batch charge agree.
    pub(crate) fn retained_owned_bytes(&self) -> usize {
        let file = self.file.as_str().len();
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
            }) => file + message.len() + gap.path.len(),
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
/// non-`Default`; its operations are exactly `new`, `push`, logical
/// `is_empty`, consuming `absorb`, `absorb_syntax`, and total `finish` (plus
/// whole-owner swap/replace where the generic lifecycle requires them).
#[derive(Debug)]
pub(crate) struct DiagnosticCollector {
    state: CollectorState,
}

#[derive(Debug)]
enum CollectorState {
    Retaining {
        count: usize,
        owned_bytes: usize,
        rows: Vec<SourceDiagnostic>,
    },
    Limited {
        count: usize,
        owned_bytes: usize,
        limit: CompileDiagnosticLimit,
    },
}

/// The finished terminal of one collector: the complete ordered payload with
/// its exact totals, or the typed limit with saturated totals and no payload.
/// Complete is empty exactly when its count is zero; Limited owns no vector.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BoundedDiagnostics {
    Complete {
        count: usize,
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
        matches!(self, BoundedDiagnostics::Complete { count: 0, .. })
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

/// A test view of a collector's exact state, for isolation and lifecycle
/// probes that must compare before/after owners without widening the
/// production operation set.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CollectorProbe {
    pub(crate) count: usize,
    pub(crate) owned_bytes: usize,
    pub(crate) limit: Option<CompileDiagnosticLimit>,
    pub(crate) rows: Vec<SourceDiagnostic>,
}

/// The saturated Limited state: totals cap at ceiling plus one, so later
/// input keeps composing without unbounded growth.
fn limited_state(
    count: usize,
    owned_bytes: usize,
    limit: CompileDiagnosticLimit,
) -> CollectorState {
    CollectorState::Limited {
        count: count.min(MAX_DIAGNOSTIC_COUNT + 1),
        owned_bytes: owned_bytes.min(MAX_DIAGNOSTIC_BYTES + 1),
        limit,
    }
}

/// Update a Limited owner's saturated totals and strengthen an OwnedBytes
/// limit to Count when the composed count crosses. Count never weakens.
fn accumulate_limited(
    count: &mut usize,
    owned_bytes: &mut usize,
    limit: &mut CompileDiagnosticLimit,
    added_count: usize,
    added_bytes: usize,
) {
    *count = count
        .saturating_add(added_count)
        .min(MAX_DIAGNOSTIC_COUNT + 1);
    *owned_bytes = owned_bytes
        .saturating_add(added_bytes)
        .min(MAX_DIAGNOSTIC_BYTES + 1);
    if matches!(limit, CompileDiagnosticLimit::OwnedBytes { .. }) && *count > MAX_DIAGNOSTIC_COUNT {
        *limit = CompileDiagnosticLimit::Count {
            limit: MAX_DIAGNOSTIC_COUNT,
        };
    }
}

/// The rows one admission carries: a single pushed row or a complete batch.
enum RowSource {
    One(SourceDiagnostic),
    Many(Vec<SourceDiagnostic>),
}

impl DiagnosticCollector {
    pub(crate) fn new() -> Self {
        Self {
            state: CollectorState::Retaining {
                count: 0,
                owned_bytes: 0,
                rows: Vec::new(),
            },
        }
    }

    /// Logical emptiness: a Limited owner retains no rows but is never empty.
    pub(crate) fn is_empty(&self) -> bool {
        match &self.state {
            CollectorState::Retaining { count, .. } => *count == 0,
            CollectorState::Limited { .. } => false,
        }
    }

    /// Retain one finalized row, charging its exact retained owned bytes.
    pub(crate) fn push(&mut self, row: SourceDiagnostic) {
        let bytes = row.retained_owned_bytes();
        self.admit(1, bytes, RowSource::One(row));
    }

    /// Merge a finished terminal into this live owner. A Complete terminal's
    /// rows append in order with their exact totals; a Limited terminal leaves
    /// this owner Limited unconditionally — its destroyed payload never
    /// re-materializes.
    pub(crate) fn absorb(&mut self, finished: BoundedDiagnostics) {
        match finished {
            BoundedDiagnostics::Complete {
                count,
                owned_bytes,
                rows,
            } => self.admit(count, owned_bytes, RowSource::Many(rows)),
            BoundedDiagnostics::Limited {
                count,
                owned_bytes,
                limit,
            } => self.absorb_limited(count, owned_bytes, limit),
        }
    }

    /// The sole syntax bridge: consume one parsed file's bounded terminal.
    /// Complete rows move into `Syntax` payloads under `file`; the batch
    /// charge is the summary's owned bytes — message, help, and reason-owned
    /// bytes, exact by construction in the syntax owner — plus one file
    /// spelling per row, which is exactly the per-row
    /// [`SourceDiagnostic::retained_owned_bytes`] sum. A Limited terminal
    /// composes the same charge from its saturated summary and leaves this
    /// owner Limited unconditionally (A2), selecting Count when both composed
    /// kinds have crossed.
    pub(crate) fn absorb_syntax(&mut self, file: &FileIdentity, diagnostics: SyntaxDiagnostics) {
        let summary = diagnostics.summary();
        let count = summary.count();
        let bytes = summary
            .owned_bytes()
            .saturating_add(count.saturating_mul(file.as_str().len()));
        match diagnostics.into_complete() {
            Ok(payload) => {
                // The syntax terminal exposes borrowing access only, so each
                // retained row is cloned into its compiler envelope; the
                // transient double-retention is bounded by one file's syntax
                // payload.
                let rows = payload
                    .as_slice()
                    .iter()
                    .map(|diagnostic| SourceDiagnostic::syntax(file, diagnostic.clone()))
                    .collect();
                self.admit(count, bytes, RowSource::Many(rows));
            }
            Err(limit) => {
                let inherited = match limit {
                    SyntaxDiagnosticLimit::Count { .. } => CompileDiagnosticLimit::Count {
                        limit: MAX_DIAGNOSTIC_COUNT,
                    },
                    SyntaxDiagnosticLimit::OwnedBytes { .. } => {
                        CompileDiagnosticLimit::OwnedBytes {
                            limit: MAX_DIAGNOSTIC_BYTES,
                        }
                    }
                };
                self.absorb_limited(count, bytes, inherited);
            }
        }
    }

    /// Seal this owner into its terminal. Total: every state has a terminal.
    pub(crate) fn finish(self) -> BoundedDiagnostics {
        match self.state {
            CollectorState::Retaining {
                count,
                owned_bytes,
                rows,
            } => BoundedDiagnostics::Complete {
                count,
                owned_bytes,
                rows,
            },
            CollectorState::Limited {
                count,
                owned_bytes,
                limit,
            } => BoundedDiagnostics::Limited {
                count,
                owned_bytes,
                limit,
            },
        }
    }

    /// Admit materialized rows with their exact totals. Crossing a ceiling
    /// discards the whole retained payload — the incoming rows and every
    /// already-retained prefix row — and Count wins a simultaneous crossing.
    fn admit(&mut self, added_count: usize, added_bytes: usize, incoming: RowSource) {
        match &mut self.state {
            CollectorState::Retaining {
                count,
                owned_bytes,
                rows,
            } => {
                let new_count = count.saturating_add(added_count);
                let new_bytes = owned_bytes.saturating_add(added_bytes);
                if new_count > MAX_DIAGNOSTIC_COUNT {
                    self.state = limited_state(
                        new_count,
                        new_bytes,
                        CompileDiagnosticLimit::Count {
                            limit: MAX_DIAGNOSTIC_COUNT,
                        },
                    );
                } else if new_bytes > MAX_DIAGNOSTIC_BYTES {
                    self.state = limited_state(
                        new_count,
                        new_bytes,
                        CompileDiagnosticLimit::OwnedBytes {
                            limit: MAX_DIAGNOSTIC_BYTES,
                        },
                    );
                } else {
                    *count = new_count;
                    *owned_bytes = new_bytes;
                    match incoming {
                        RowSource::One(row) => rows.push(row),
                        RowSource::Many(mut batch) => rows.append(&mut batch),
                    }
                }
            }
            CollectorState::Limited {
                count,
                owned_bytes,
                limit,
            } => accumulate_limited(count, owned_bytes, limit, added_count, added_bytes),
        }
    }

    /// Compose an absorbed Limited terminal: this owner becomes (or stays)
    /// Limited unconditionally, even when the composed totals sit under both
    /// ceilings — the absorbed payload was destroyed and never
    /// re-materializes. A composed crossing selects its own kind, Count first;
    /// otherwise the absorbed limit kind is inherited.
    fn absorb_limited(
        &mut self,
        added_count: usize,
        added_bytes: usize,
        inherited: CompileDiagnosticLimit,
    ) {
        match &mut self.state {
            CollectorState::Retaining {
                count, owned_bytes, ..
            } => {
                let new_count = count.saturating_add(added_count);
                let new_bytes = owned_bytes.saturating_add(added_bytes);
                let limit = if new_count > MAX_DIAGNOSTIC_COUNT {
                    CompileDiagnosticLimit::Count {
                        limit: MAX_DIAGNOSTIC_COUNT,
                    }
                } else if new_bytes > MAX_DIAGNOSTIC_BYTES {
                    CompileDiagnosticLimit::OwnedBytes {
                        limit: MAX_DIAGNOSTIC_BYTES,
                    }
                } else {
                    inherited
                };
                self.state = limited_state(new_count, new_bytes, limit);
            }
            CollectorState::Limited {
                count,
                owned_bytes,
                limit,
            } => accumulate_limited(count, owned_bytes, limit, added_count, added_bytes),
        }
    }

    /// Test view of the exact owner state.
    #[cfg(test)]
    pub(crate) fn probe(&self) -> CollectorProbe {
        match &self.state {
            CollectorState::Retaining {
                count,
                owned_bytes,
                rows,
            } => CollectorProbe {
                count: *count,
                owned_bytes: *owned_bytes,
                limit: None,
                rows: rows.clone(),
            },
            CollectorState::Limited {
                count,
                owned_bytes,
                limit,
            } => CollectorProbe {
                count: *count,
                owned_bytes: *owned_bytes,
                limit: Some(*limit),
                rows: Vec::new(),
            },
        }
    }

    /// Test view of the retained rows (empty once Limited).
    #[cfg(test)]
    pub(crate) fn probe_rows(&self) -> &[SourceDiagnostic] {
        match &self.state {
            CollectorState::Retaining { rows, .. } => rows,
            CollectorState::Limited { .. } => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use marrow_syntax::{SYNTAX_DIAGNOSTIC_COUNT_LIMIT, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT};

    /// A7: the compiler ceilings equal the syntax ceilings. The syntax
    /// bridge's Limited composition depends on this equality; a divergence is
    /// a deliberate contract change that must update both rows together.
    #[test]
    fn compiler_ceilings_equal_the_syntax_ceilings() {
        assert_eq!(MAX_DIAGNOSTIC_COUNT, SYNTAX_DIAGNOSTIC_COUNT_LIMIT);
        assert_eq!(MAX_DIAGNOSTIC_BYTES, SYNTAX_DIAGNOSTIC_OWNED_BYTES_LIMIT);
    }

    fn file() -> &'static FileIdentity {
        crate::test_main_file_identity()
    }

    /// A rendered compiler row whose retained owned bytes are exactly
    /// `file.len() + message_len`.
    fn row_with_message_len(message_len: usize) -> SourceDiagnostic {
        SourceDiagnostic::at(
            "check.type",
            file(),
            SourceSpan::default(),
            "x".repeat(message_len),
        )
    }

    /// The byte-charge law, per payload variant: file spelling plus owned
    /// message, syntax help, and identity-gap path bytes; the static
    /// invalid-UTF-8 message charges nothing beyond the file.
    #[test]
    fn retained_owned_bytes_charges_each_payload_component_exactly() {
        let file_len = file().as_str().len();

        let rendered = row_with_message_len(10);
        assert_eq!(rendered.retained_owned_bytes(), file_len + 10);

        let gap = SourceDiagnostic::with_identity_gap(
            "check.durable_identity",
            file(),
            SourceSpan::default(),
            "y".repeat(7),
            IdentityGap {
                kind: IdentityKind::Root,
                path: "^books".to_string(),
                retired: false,
            },
        );
        assert_eq!(gap.retained_owned_bytes(), file_len + 7 + "^books".len());

        let utf8 = SourceDiagnostic::invalid_utf8(file(), 3, Some(1));
        assert_eq!(utf8.retained_owned_bytes(), file_len);
        assert_eq!(utf8.invalid_utf8_facts(), Some((3, Some(1))));
        assert_eq!(utf8.message(), "source file is not valid UTF-8");
        assert_eq!(utf8.code(), "check.unsupported");
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

    /// Exact count edges: the 4096th row retains; the 4097th destroys the
    /// whole payload (prefix included) for a Count limit with saturated
    /// totals, and the owner is logically non-empty forever after.
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
        let file_len = file().as_str().len();
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
        assert_eq!(merged.owned_bytes, 3 * file().as_str().len() + 1 + 3 + 5);
        assert_eq!(
            merged.rows[1].retained_owned_bytes(),
            file().as_str().len() + 3
        );

        // A Limited terminal forces Limited; with the composed count crossed,
        // Count is selected over the absorbed OwnedBytes kind.
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

    /// The syntax bridge charges the summary's owned bytes plus one file
    /// spelling per row — so a longer file identity crosses the byte ceiling
    /// where a shorter one retains, at the exact edge.
    #[test]
    fn absorb_syntax_multiplies_the_file_spelling_by_the_row_count() {
        let source = "@\n@\n@\n@\n";
        let summary = marrow_syntax::parse_source(source).diagnostics.summary();
        let short = crate::test_file_identity("src/a.mw");
        let long = crate::test_file_identity("src/abcdefgh.mw");
        assert_eq!(long.as_str().len(), short.as_str().len() + 7);

        // Fill so that absorbing under the short path lands exactly at the
        // ceiling and under the long path crosses it.
        let batch = |identity: &FileIdentity| {
            summary.owned_bytes() + summary.count() * identity.as_str().len()
        };
        let prefill = MAX_DIAGNOSTIC_BYTES - batch(&short) - file().as_str().len();

        let mut exact = DiagnosticCollector::new();
        exact.push(row_with_message_len(prefill));
        exact.absorb_syntax(&short, marrow_syntax::parse_source(source).diagnostics);
        let at_edge = exact.probe();
        assert_eq!(at_edge.owned_bytes, MAX_DIAGNOSTIC_BYTES);
        assert_eq!(at_edge.limit, None);
        assert_eq!(at_edge.count, 1 + summary.count());

        let mut crossing = DiagnosticCollector::new();
        crossing.push(row_with_message_len(prefill));
        crossing.absorb_syntax(&long, marrow_syntax::parse_source(source).diagnostics);
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
            summary.owned_bytes() + 2 * file().as_str().len()
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
        assert!(probe.rows.iter().all(|row| row.file() == file()));
    }

    /// A2 at the bridge: absorbing a Limited syntax terminal leaves the
    /// collector Limited even though it retained nothing itself, and the
    /// destroyed payload never re-materializes through later input.
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
