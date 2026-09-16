//! The editor analysis fact floor: one immutable, revisioned [`AnalysisSnapshot`] per
//! exact project input.
//!
//! A caller hands [`analyze`] the exact [`ProjectInput`] it wants analyzed and an
//! [`InputRevision`] it assigns. The revision labels which input a result belongs to;
//! the floor echoes it and never treats it as content identity or an ordering key. The
//! snapshot enumerates the complete, resilient diagnostic set — every stage's
//! diagnostics over every module, so an independent valid component keeps its
//! diagnostics even when a sibling fails to parse — and shares the caller's
//! `Arc<ProjectInput>` without copying its bytes.
//!
//! An outcome that is not a truthful diagnostic set is a typed failure, never a
//! diagnostic: an aggregate resource bound is [`AnalysisFailure::ResourceLimit`] and an
//! opaque compiler-coherence failure is [`AnalysisFailure::Invariant`], each echoing the
//! caller revision. The shared precedence is `Invariant > Diagnostics > ResourceLimit`.

use std::sync::Arc;

use marrow_project::{CaptureLimits, FileIdentity, ProjectInput};
use marrow_syntax::{Declaration, EnumMember, FormatRefusal, SourceSpan};

use crate::compile::{Analyzed, analyze_project};

mod active_call;
mod completion;
mod facts;

use crate::{CompileInvariant, CompileResourceLimit, SourceDiagnostic};
pub(crate) use facts::{AnalysisFactCollector, BodySite, FactSink, ReleasedBody, StagedBodyTxn};

/// A caller-assigned revision echoed by every analysis outcome. It labels which input a
/// result belongs to; the floor never treats it as content identity, a cache key, or an
/// ordering relation. Two analyses of byte-identical inputs at different revisions are
/// distinct outcomes that each echo their own revision.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct InputRevision(u64);

impl InputRevision {
    /// A revision from a caller-assigned value.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The caller-assigned value.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// The largest number of retained facts a snapshot admits before the collection is
/// discarded as a [`AnalysisResourceLimit::SnapshotFactCount`]. Hover and definition
/// facts attach per call site and per local/parameter use site; sized at eight times the
/// image site family so it clears any real edit while failing a fact avalanche closed.
pub const MAX_SNAPSHOT_FACT_COUNT: u64 = 65_536;

/// The largest total rendered-fact byte footprint a snapshot admits before the
/// collection is discarded as a [`AnalysisResourceLimit::SnapshotFactBytes`]. A flat
/// allocation guard: four times the diagnostic-byte ceiling gives headroom for
/// nested-generic type displays without unbounded retention.
pub const MAX_SNAPSHOT_FACT_BYTES: u64 = 4 * 1024 * 1024;

/// The largest number of in-scope completion candidates one query assembles before it is
/// refused as a query-local [`AnalysisResourceLimit::CompletionCandidateCount`]. The
/// candidate set is the complete in-scope namespace for the position class — never
/// prefix-filtered, ranked, or truncated — so an over-cap namespace is a typed refusal.
/// Candidate sets are never retained per position.
pub const MAX_COMPLETION_CANDIDATES: u64 = 512;

/// The largest total rendered-candidate byte footprint one completion query assembles
/// (each candidate's label plus its detail) before it is refused as a query-local
/// [`AnalysisResourceLimit::CompletionRenderBytes`]. A query-local expansion guard, not a
/// retained snapshot bound.
pub const MAX_COMPLETION_RENDER_BYTES: u64 = 256 * 1024;

/// The largest total rendered byte footprint one active-call query assembles (the callee
/// signature display plus every parameter piece) before it is refused as a query-local
/// [`AnalysisResourceLimit::ActiveCallRenderBytes`]. The callee's parameter arity is
/// already bounded by the compiler's declaration bounds, so this guards the rendered
/// display alone, not a retained snapshot bound.
pub const MAX_ACTIVE_CALL_RENDER_BYTES: u64 = 64 * 1024;

/// The largest checked whole-document format output one query returns before it is
/// refused as a query-local outcome (never retained). The formatter's input is already
/// bounded by the pure owner's per-file admission, so this is an expansion guard, not a
/// second input bound.
pub(crate) const MAX_FORMAT_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;

/// The largest number of declaration-hierarchy symbols one module file admits before that
/// file's outline becomes [`Unavailability::Bounded`]. Every projected node — each
/// top-level declaration and each nested enum member — counts once. No partial or
/// truncated outline is retained, and no other file and no other query is affected.
pub const MAX_DOCUMENT_SYMBOLS_PER_FILE: u64 = 4_096;

/// The largest declaration-hierarchy nesting depth one module file admits before that
/// file's outline becomes [`Unavailability::Bounded`]. Top-level declarations sit at
/// depth one; enum members deepen the tree by one level each. The parser admits far
/// deeper nesting, so this bound is reachable and fails a pathological outline closed
/// rather than recursing without limit.
pub const MAX_SYMBOL_DEPTH: u16 = 16;

/// One retained fact's file, as a position in the snapshot's own
/// [`ProjectInput::modules`] order.
///
/// It is not an identity, a table, or a ledger: it is a coordinate that only the
/// snapshot which minted it can resolve, through its private `identity_of`. The drive
/// mints one per module while iterating that same order, carrying out of admission the
/// proof that the project holds at most 4096 modules before the first fact allocates, so
/// the domain is in range by construction.
///
/// The compaction is load-bearing, not cosmetic: a [`FileIdentity`] is an owned spelling
/// of up to 4096 bytes, so one clone per retained fact would be up to 256 MiB of
/// retention at the pinned fact-count ceiling. The *logical* charge is unaffected —
/// [`AnalysisFactCollector`] charges a definition target's file spelling and a
/// document-symbol module's owner spelling either way.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct FileRef(u16);

/// The admission ceiling is inside the coordinate domain, so a position in an admitted
/// project's module order is a coordinate without a fallible conversion. A widened file
/// ceiling must widen [`FileRef`] with it, and fails to build until it does.
const _: () = assert!(CaptureLimits::DEFAULT.max_files() <= u16::MAX as usize);

impl FileRef {
    /// The coordinate for the `index`-th module of a project the drive has admitted.
    /// Total: admission proved the module count is at most `max_files`, which the
    /// assertion above proves is inside this domain.
    pub(crate) fn admitted(index: u16) -> Self {
        Self(index)
    }

    /// The coordinate for the module at `index` in an iteration of
    /// [`ProjectInput::modules`], or `None` when the position is outside the domain —
    /// which resolving a caller-supplied file against an admitted snapshot answers as an
    /// unknown file. Private to the crate.
    pub(crate) fn at(index: usize) -> Option<Self> {
        u16::try_from(index).ok().map(Self)
    }

    fn index(self) -> usize {
        self.0 as usize
    }

    /// Resolve this coordinate against the project whose module order minted it.
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn of(self, project: &ProjectInput) -> &FileIdentity {
        project.modules()[self.index()].identity()
    }
}

/// One retained span, in the coordinate domain the project owner already admits.
///
/// A snapshot's facts only ever span files that passed drive admission, which refuses
/// any file over `CaptureLimits::DEFAULT`'s 1 MiB per-file ceiling, so every retained
/// offset is far inside `u32`; a test pins the two domains together.
///
/// Retained spans dominate the snapshot's structural footprint — four per hover fact and
/// its target, four per document-symbol node — so carrying the source owner's 64-bit
/// offsets in retained state would cost megabytes to represent a megabyte of positions.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct FactSpan {
    start: u32,
    end: u32,
    line: u32,
    column: u32,
}

impl FactSpan {
    fn of(span: SourceSpan) -> Self {
        // A saturating offset would collapse a span to `start == end`, making `contains`
        // always false and every fact at it silently absent. Asserting the domain makes
        // a widened admission ceiling a failing debug assertion here rather than facts
        // that quietly stop resolving. No admitted span reaches the saturating branch:
        // the drive refuses a file past `MAX_PARSED_FILE_BYTES`, orders below `u32::MAX`.
        debug_assert!(
            span.end_byte <= u32::MAX as usize,
            "a span leaves the domain"
        );
        Self {
            start: span.start_byte.min(u32::MAX as usize) as u32,
            end: span.end_byte.min(u32::MAX as usize) as u32,
            line: span.line,
            column: span.column,
        }
    }

    fn source(self) -> SourceSpan {
        SourceSpan {
            start_byte: self.start as usize,
            end_byte: self.end as usize,
            line: self.line,
            column: self.column,
        }
    }

    /// Whether `offset` lies in this span, half-open as every fact query resolves it.
    fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
}

/// A fixed analysis resource bound that produced no snapshot. It wraps the compiler's
/// [`CompileResourceLimit`] verbatim for a compile-side aggregate bound, and names the
/// snapshot fact bounds directly. Closed and exhaustively matchable.
pub enum AnalysisResourceLimit {
    /// A compile-side aggregate bound: an image count/byte ceiling, or the diagnostic
    /// count/byte ceiling on the complete analysis diagnostic set.
    Compile(CompileResourceLimit),
    /// The retained fact count exceeded [`MAX_SNAPSHOT_FACT_COUNT`].
    SnapshotFactCount { limit: u64 },
    /// The retained fact byte footprint exceeded [`MAX_SNAPSHOT_FACT_BYTES`].
    SnapshotFactBytes { limit: u64 },
    /// One completion query's in-scope candidate set exceeded
    /// [`MAX_COMPLETION_CANDIDATES`]. A query-local refusal, never a truncated prefix.
    CompletionCandidateCount { limit: u64 },
    /// One completion query's rendered candidate byte footprint exceeded
    /// [`MAX_COMPLETION_RENDER_BYTES`]. A query-local refusal.
    CompletionRenderBytes { limit: u64 },
    /// One active-call query's rendered signature-and-parameter byte footprint exceeded
    /// [`MAX_ACTIVE_CALL_RENDER_BYTES`]. A query-local refusal.
    ActiveCallRenderBytes { limit: u64 },
}

impl AnalysisResourceLimit {
    /// The sentence fragment a person reads for the exhausted bound, lowercase and
    /// unpunctuated. A compile-side bound answers in
    /// [`ResourceLimitKind`](crate::ResourceLimitKind)'s own words, so one bound reads
    /// the same whichever owner reports it.
    pub fn description(&self) -> &'static str {
        match self {
            AnalysisResourceLimit::Compile(limit) => limit.kind().description(),
            AnalysisResourceLimit::SnapshotFactCount { .. } => "the analysis fact table is full",
            AnalysisResourceLimit::SnapshotFactBytes { .. } => {
                "the analysis facts hold too much text to retain"
            }
            AnalysisResourceLimit::CompletionCandidateCount { .. } => {
                "one completion query has too many candidates"
            }
            AnalysisResourceLimit::CompletionRenderBytes { .. } => {
                "one completion query renders too much text"
            }
            AnalysisResourceLimit::ActiveCallRenderBytes { .. } => {
                "one signature query renders too much text"
            }
        }
    }
}

/// Why analysis produced no snapshot. Both arms echo the caller revision exactly and
/// carry no source-shaped payload. `Invariant` dominates a diagnostic set; a resource
/// limit surfaces only when no invariant and no complete diagnostic set exist.
pub enum AnalysisFailure {
    /// A fixed aggregate resource bound was exhausted.
    ResourceLimit {
        revision: InputRevision,
        limit: AnalysisResourceLimit,
    },
    /// Private compiler state was incoherent; the cause is opaque.
    Invariant {
        revision: InputRevision,
        invariant: CompileInvariant,
    },
}

impl AnalysisFailure {
    /// The caller revision this failure echoes.
    pub fn revision(&self) -> InputRevision {
        match self {
            Self::ResourceLimit { revision, .. } | Self::Invariant { revision, .. } => *revision,
        }
    }
}

/// An immutable analysis snapshot: the exact input it was computed from, the caller
/// revision, and the complete diagnostic set for the project in compiler order. The
/// input is the caller's same `Arc<ProjectInput>`, shared not copied, so a clone is O(1)
/// and the source bytes are charged once.
///
/// Every retained collection is a boxed slice: a snapshot is immutable, so the growth
/// capacity an amortized `Vec` carries is not part of its retained state.
pub struct AnalysisSnapshot {
    input: Arc<ProjectInput>,
    revision: InputRevision,
    diagnostics: Box<[SourceDiagnostic]>,
    hover_facts: Box<[HoverFact]>,
    /// The input files that did not parse. A hover query in one of these is
    /// [`Unavailability::Syntax`], not `Absent`.
    broken_files: Box<[FileRef]>,
    /// `(file, callee span)` for qualified calls whose target module did not parse. A
    /// query at one of these positions is [`Unavailability::Dependency`], not `Absent`.
    dependency_gaps: Box<[(FileRef, FactSpan)]>,
    /// The declaration-hierarchy outline of each cleanly-parsed module file, in source
    /// declaration order. A file that did not parse has no entry, and a
    /// `document_symbols` query for it is [`Unavailability::Syntax`], not an absent tree.
    document_symbols: Box<[(FileRef, Box<[DeclSymbol]>)]>,
    /// Files whose outline crossed [`MAX_DOCUMENT_SYMBOLS_PER_FILE`] or
    /// [`MAX_SYMBOL_DEPTH`]. Nothing is retained for such a file, so its
    /// `document_symbols` is [`Unavailability::Bounded`] and no other query is affected.
    symbol_bounded_files: Box<[FileRef]>,
}

impl AnalysisSnapshot {
    /// The caller revision this snapshot echoes.
    pub fn revision(&self) -> InputRevision {
        self.revision
    }

    /// The exact project input this snapshot was computed from.
    pub fn input(&self) -> &Arc<ProjectInput> {
        &self.input
    }

    /// Every diagnostic in the project, across every module and stage, in compiler
    /// order.
    pub fn diagnostics(&self) -> &[SourceDiagnostic] {
        &self.diagnostics
    }

    /// The diagnostics that point into `file`, in compiler order. Empty when the file is
    /// clean — a truthful empty list, not an absent one.
    pub fn diagnostics_for<'a>(
        &'a self,
        file: &'a FileIdentity,
    ) -> impl Iterator<Item = &'a SourceDiagnostic> + 'a {
        self.diagnostics
            .iter()
            .filter(move |diagnostic| diagnostic.file() == file)
    }

    /// The one coordinate validator: resolve an input file to its snapshot-local
    /// [`FileRef`] and its source bytes, or a typed query error when the file is not one
    /// of the snapshot's analyzed inputs. Every fact query resolves through here, so a
    /// fact can only ever index bytes this snapshot holds.
    fn locate(&self, file: &FileIdentity) -> Result<(FileRef, &[u8]), QueryError> {
        self.input
            .modules()
            .iter()
            .enumerate()
            .find(|(_, module)| module.identity() == file)
            .and_then(|(index, module)| FileRef::at(index).map(|at| (at, module.source())))
            .ok_or(QueryError::UnknownFile)
    }

    /// Resolve a coordinate this snapshot minted back to the file it names. Drive
    /// admission bounds the module count below the coordinate domain, so every
    /// retained coordinate names a module of this snapshot's own input.
    fn identity_of(&self, file: FileRef) -> Option<&FileIdentity> {
        self.input
            .modules()
            .get(file.index())
            .map(|module| module.identity())
    }

    /// Whether an offset falls in a dependency-gap span for `file` — a qualified call
    /// whose target module did not parse, so the fact is unavailable, not absent.
    fn dependency_gap_at(&self, file: FileRef, offset: u32) -> bool {
        self.dependency_gaps
            .iter()
            .any(|(gap_file, span)| *gap_file == file && span.contains(offset))
    }

    /// The hover fact at a byte offset in a file: the canonical type display of the
    /// resolved local or parameter use, or the resolved-function signature of a call
    /// callee, spanning the offset. An unknown file or an out-of-range offset is a typed
    /// [`QueryError`]; a position in a module that did not parse is
    /// [`Unavailability::Syntax`]; a call to a module that did not parse is
    /// [`Unavailability::Dependency`]; a valid position with no fact is `Absent`.
    ///
    /// A position inside a generic function's template body carries facts too: they are
    /// collected once at the template (never per instance), and a template-parameter use
    /// renders by its declared spelling.
    pub fn hover(&self, file: &FileIdentity, offset: usize) -> Result<Fact<Hover>, QueryError> {
        let (file, source) = self.locate(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        if self.broken_files.contains(&file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        let offset = offset as u32;
        if self.dependency_gap_at(file, offset) {
            return Ok(Fact::Unavailable(Unavailability::Dependency));
        }
        match self.fact_at(file, offset) {
            Some(fact) => Ok(Fact::Present(Hover {
                display: fact.display.to_string(),
            })),
            None => Ok(Fact::Absent),
        }
    }

    /// The first retained fact spanning `offset` in `file`, in collection order.
    fn fact_at(&self, file: FileRef, offset: u32) -> Option<&HoverFact> {
        self.hover_facts
            .iter()
            .find(|fact| fact.file == file && fact.span.contains(offset))
    }

    /// The definition target at a byte offset: for a resolved function callee spanning
    /// the offset, the file, declared-name span, and header-through-body range of its
    /// target. An unknown file or an out-of-range offset is a typed [`QueryError`]; a
    /// position in a module that did not parse is [`Unavailability::Syntax`]; a position
    /// with no callee fact (a local use, a literal, whitespace) is `Absent`.
    ///
    /// Definition covers source-defined function callees, including a call inside a
    /// generic template body (collected once at the template), and a generic call targets
    /// its source template. Local, type, import, and field definitions are not covered.
    pub fn definition(
        &self,
        file: &FileIdentity,
        offset: usize,
    ) -> Result<Fact<Definition>, QueryError> {
        let (file, source) = self.locate(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        if self.broken_files.contains(&file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        let offset = offset as u32;
        if self.dependency_gap_at(file, offset) {
            return Ok(Fact::Unavailable(Unavailability::Dependency));
        }
        match self.fact_at(file, offset).and_then(|fact| fact.definition) {
            // A retained target always names a module of this snapshot's own input,
            // so an unresolvable coordinate is not absence — it is no fact at all.
            Some(target) => match self.identity_of(target.file) {
                Some(identity) => Ok(Fact::Present(Definition {
                    file: identity.clone(),
                    name_span: target.name_span.source(),
                    declaration_range: target.decl_range.source(),
                })),
                None => Ok(Fact::Absent),
            },
            None => Ok(Fact::Absent),
        }
    }

    /// The checked whole-document format of an input file. Consumes the one
    /// syntax-owned [`marrow_syntax::check_format`] policy — the same the CLI's
    /// `marrow fmt` uses — so the refusal decision is classified once. The output is
    /// bounded by [`MAX_FORMAT_OUTPUT_BYTES`] as a query-local refusal (never retained
    /// in the snapshot). An unknown file is a typed [`QueryError`].
    pub fn format(&self, file: &FileIdentity) -> Result<FormatOutcome, QueryError> {
        let (_, source) = self.locate(file)?;
        let Ok(source) = std::str::from_utf8(source) else {
            // A non-UTF-8 file cannot be lexed. A parse-invalid refusal carries
            // real nonempty syntax evidence, which an undecodable file has
            // none of, so the outcome is its own typed arm.
            return Ok(FormatOutcome::InvalidUtf8);
        };
        match marrow_syntax::check_format(source) {
            Ok(formatted) if formatted.len() as u64 > MAX_FORMAT_OUTPUT_BYTES => {
                Ok(FormatOutcome::TooLarge {
                    limit: MAX_FORMAT_OUTPUT_BYTES,
                })
            }
            Ok(formatted) => Ok(FormatOutcome::Formatted(formatted)),
            Err(refusal) => Ok(FormatOutcome::Refused(refusal)),
        }
    }

    /// The declaration-hierarchy outline of a module file: its top-level declarations in
    /// source order, each nested enum member under its enum, projected from the parsed
    /// AST's existing declared-name spans and declaration ranges. An unknown file is a
    /// typed [`QueryError`]; a file that did not parse is [`Unavailability::Syntax`]; a
    /// cleanly-parsed file whose outline crossed [`MAX_DOCUMENT_SYMBOLS_PER_FILE`] or
    /// [`MAX_SYMBOL_DEPTH`] is [`Unavailability::Bounded`], with nothing partial retained
    /// for it; a cleanly-parsed file with no declarations is a truthful `Present` empty
    /// outline.
    ///
    /// A pure projection: it reclassifies nothing and reads no resolved semantic
    /// identity. The outline is retained per snapshot and bounded per file at snapshot
    /// admission; the bound refuses that file's outline alone, never the snapshot.
    pub fn document_symbols(&self, file: &FileIdentity) -> Result<Fact<&[DeclSymbol]>, QueryError> {
        let (file, _) = self.locate(file)?;
        if self.broken_files.contains(&file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        if self.symbol_bounded_files.contains(&file) {
            return Ok(Fact::Unavailable(Unavailability::Bounded));
        }
        match self
            .document_symbols
            .iter()
            .find(|(symbol_file, _)| *symbol_file == file)
        {
            Some((_, symbols)) => Ok(Fact::Present(symbols)),
            // A validated input that is neither broken nor retained did not parse
            // cleanly: syntax-unavailable, never a fabricated empty tree.
            None => Ok(Fact::Unavailable(Unavailability::Syntax)),
        }
    }

    /// The completion classification and candidate namespace at a byte offset in a file.
    ///
    /// The position class is derived purely positionally from the checker's resolution
    /// model over a parse of this file's own retained bytes — never from the trigger
    /// character, document text, or a token scan. The candidate set is the complete
    /// in-scope namespace for the class, as [`PositionClass`] enumerates it.
    ///
    /// The set is never prefix-filtered, ranked, or truncated: an over-cap namespace is a
    /// query-local [`CompletionOutcome::Refused`]. The parse and the re-resolution over
    /// it are per query and transient — no parse tree and no per-position candidate set
    /// is retained.
    ///
    /// An unknown file or an out-of-range offset is a typed [`QueryError`]. A file that
    /// produced no parse tree (a non-UTF-8 file) is [`Unavailability::Syntax`]. A broken
    /// file still classifies: a position over a recovered incomplete form (`base.`,
    /// `Enum::`) yields its class and candidates even though the file has parse errors.
    /// A position with no class (a literal, a comment, whitespace outside any recovered
    /// node) is `Absent`.
    ///
    /// The traversal is strictly read-only: it never drives the compile-path lowerer or
    /// resolver, so a partial or malformed base yields an `Absent`/empty classification
    /// and leaks no diagnostic into the snapshot.
    pub fn completions(
        &self,
        file: &FileIdentity,
        offset: usize,
    ) -> Result<CompletionOutcome, QueryError> {
        let (_, source) = self.locate(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        let Some(tree) = query_local_parse(source, offset) else {
            // A validated input file that cannot be decoded never produced a tree:
            // syntax-unavailable, never a fabricated empty set.
            return Ok(CompletionOutcome::Ready(Fact::Unavailable(
                Unavailability::Syntax,
            )));
        };
        Ok(completion::resolve(&QueryFile::new(&tree)))
    }

    /// The active-call fact at a byte offset: the innermost enclosing call's callee
    /// signature, its parameter pieces, and the active argument index the offset sits at.
    ///
    /// The enclosing call and active index are derived purely positionally over a parse
    /// of this file's own retained bytes — never from the trigger character or a
    /// document-text scan. The callee resolves to a same-module function or generic
    /// template declared in the file, and a generic callee presents its source template
    /// signature. Parameter pieces are rendered separately from the declared spellings,
    /// so a consumer marks the active one without substring-searching the display.
    ///
    /// An unknown file or an out-of-range offset is a typed [`QueryError`]. A file that
    /// produced no parse tree (a non-UTF-8 file) is [`Unavailability::Syntax`]. A broken
    /// file still resolves: a recovered incomplete-call node yields its active-call fact
    /// even though the file has parse errors. A position in no call, or a call whose callee
    /// resolves to no local declaration (a built-in, a cross-module callee, or an unknown
    /// name), is `Absent`. An over-cap rendered display is a query-local
    /// [`ActiveCallOutcome::Refused`], never a truncated display.
    pub fn active_call(
        &self,
        file: &FileIdentity,
        offset: usize,
    ) -> Result<ActiveCallOutcome, QueryError> {
        let (_, source) = self.locate(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        let Some(tree) = query_local_parse(source, offset) else {
            // A validated input file that cannot be decoded never produced a tree:
            // syntax-unavailable, never a fabricated absence.
            return Ok(ActiveCallOutcome::Ready(Fact::Unavailable(
                Unavailability::Syntax,
            )));
        };
        Ok(active_call::resolve(&QueryFile::new(&tree), source))
    }
}

/// Parse exactly one already-admitted file's already-retained bytes for one query.
///
/// The tree is transient: it is never retained, never enters a collector, and
/// contributes no diagnostic. `broken_files` stays the independent record of
/// parseability — no query infers parseability from this parse, and a recovered broken
/// file still classifies positions over its recovered forms. Syntax retains every
/// declaration header but materializes statements only in the containing function/test
/// body, and the bound position travels with that partial syntax.
///
/// Its peak is charged before it is incurred, by an owner that runs before any file is
/// parsed: [`crate::MAX_PARSED_FILE_BYTES`] is the longest file drive admission accepts,
/// derived from [`crate::MAX_QUERY_PARSE_TRANSIENT_BYTES`] and the rate `marrow-syntax`
/// publishes for the representation it builds. Every file a snapshot holds therefore has
/// an accounted parse charge under that ceiling, so a refusal arm here would be
/// unreachable — a claim no test could keep honest.
fn query_local_parse(source: &[u8], offset: usize) -> Option<marrow_syntax::QuerySyntax> {
    let source = std::str::from_utf8(source).ok()?;
    Some(marrow_syntax::QuerySyntax::parse(source, offset))
}

struct QueryFile<'a> {
    declarations: &'a [marrow_syntax::Declaration],
    uses: &'a [marrow_syntax::UseDecl],
    offset: u32,
}

#[cfg(test)]
#[path = "analysis/query_tests.rs"]
mod query_tests;

impl<'a> QueryFile<'a> {
    fn new(syntax: &'a marrow_syntax::QuerySyntax) -> Self {
        Self {
            declarations: syntax.declarations(),
            uses: syntax.uses(),
            offset: syntax.offset() as u32,
        }
    }
}

/// The outcome of a checked whole-document format query.
pub enum FormatOutcome {
    /// The canonical formatted source.
    Formatted(String),
    /// Formatting was refused by the syntax-owned policy (unparsed source, or comment
    /// loss).
    Refused(FormatRefusal),
    /// The formatted output exceeded [`MAX_FORMAT_OUTPUT_BYTES`]; a query-local refusal,
    /// not retained.
    TooLarge { limit: u64 },
    /// The file is not valid UTF-8, so it cannot be lexed at all — distinct
    /// from a parse-invalid refusal, which carries nonempty syntax evidence.
    InvalidUtf8,
}

/// The definition target of a resolved function callee: the file the target is declared
/// in, the span of its declared name (the selection range), and the full
/// header-through-body declaration range. A generic call targets its source template.
pub struct Definition {
    file: FileIdentity,
    name_span: marrow_syntax::SourceSpan,
    declaration_range: marrow_syntax::SourceSpan,
}

impl Definition {
    /// The file the target is declared in.
    pub fn file(&self) -> &FileIdentity {
        &self.file
    }

    /// The span of the target's declared name — the selection range.
    pub fn name_span(&self) -> marrow_syntax::SourceSpan {
        self.name_span
    }

    /// The full header-through-body declaration range of the target.
    pub fn declaration_range(&self) -> marrow_syntax::SourceSpan {
        self.declaration_range
    }
}

/// Analyze one exact project input at a caller-assigned revision, producing an immutable
/// snapshot or a typed failure. Whole-project recomputation: the analysis runs the same
/// resilient driver the production compile uses, includes test bodies, and echoes the
/// caller revision on every outcome.
pub fn analyze(
    input: Arc<ProjectInput>,
    revision: InputRevision,
) -> Result<Arc<AnalysisSnapshot>, AnalysisFailure> {
    let analysis = analyze_project(&input).map_err(|limit| AnalysisFailure::ResourceLimit {
        revision,
        limit: AnalysisResourceLimit::Compile(limit),
    })?;
    let diagnostics = match analysis.outcome {
        Analyzed::Invariant(invariant) => {
            return Err(AnalysisFailure::Invariant {
                revision,
                invariant,
            });
        }
        Analyzed::ResourceLimit(limit) => {
            return Err(AnalysisFailure::ResourceLimit {
                revision,
                limit: AnalysisResourceLimit::Compile(limit),
            });
        }
        Analyzed::Diagnostics(diagnostics) => diagnostics.into_vec(),
        // The snapshot publishes facts, never an image: the checked program is dropped
        // here without encoding, so no image-policy bound is reachable from analysis.
        Analyzed::Checked(_) => Vec::new(),
    };
    // The fact ledger admitted every fact against its ceilings at the push, so the
    // sealed terminal is either the complete retained set or the typed limit that
    // discarded it. No partial fact set is ever published.
    let facts = match analysis.facts {
        BoundedAnalysisFacts::Complete(facts) => facts,
        BoundedAnalysisFacts::Limited { limit } => {
            return Err(AnalysisFailure::ResourceLimit {
                revision,
                limit: fact_limit_failure(limit),
            });
        }
    };
    let RetainedFacts {
        hover_facts,
        broken_files,
        dependency_gaps,
        document_symbols,
    } = facts;
    Ok(Arc::new(AnalysisSnapshot {
        input,
        revision,
        diagnostics: diagnostics.into_boxed_slice(),
        hover_facts,
        broken_files,
        dependency_gaps,
        document_symbols,
        symbol_bounded_files: analysis.symbol_bounded_files,
    }))
}

/// Map the ledger's typed ceiling to its public resource-limit record: the one
/// failure-boundary translation, exhaustive over both kinds. The ledger's saturated
/// count and byte totals stay internal — a published saturated total would be exactly
/// the fabricated count the typed limits exist to prevent.
fn fact_limit_failure(limit: AnalysisFactLimit) -> AnalysisResourceLimit {
    match limit {
        AnalysisFactLimit::Count { limit } => AnalysisResourceLimit::SnapshotFactCount { limit },
        AnalysisFactLimit::Bytes { limit } => AnalysisResourceLimit::SnapshotFactBytes { limit },
    }
}

/// One retained editor fact: a resolved local or parameter use site and the canonical
/// display of its value type. Held per snapshot and queried by [`AnalysisSnapshot::hover`].
///
/// Private to this module, not `pub(crate)`: a producer outside the ledger cannot name
/// the type, so it cannot declare a field or a parameter that carries hover facts in
/// bulk — bulk staging is unrepresentable rather than merely scanned for. Producers
/// reach the ledger through [`FactSink::hover`], which takes the parts and admits at
/// the push.
struct HoverFact {
    file: FileRef,
    span: FactSpan,
    display: Box<str>,
    /// The definition target when this fact is a resolved function callee; `None` for a
    /// local or parameter use.
    ///
    /// Carried inline: every coordinate in it is compact, so inlining costs the accounted
    /// worst case less than a second retained table plus a reference into it, and the
    /// snapshot keeps one retained fact family instead of two.
    definition: Option<DefinitionTarget>,
}

impl HoverFact {
    /// The logical byte charge of one retained hover fact: its display spelling plus the
    /// file spelling of an optional definition target, which `spelling` resolves for a
    /// coordinate. Fixed-size fields are charged by the count bound.
    ///
    /// The destructure is exhaustive so a new heap-owning field on this retained type is
    /// a build error here rather than retention the exported term never saw.
    fn retained_bytes(&self, spelling: impl FnOnce(FileRef) -> u64) -> u64 {
        let HoverFact {
            file: _,
            span: _,
            display,
            definition,
        } = self;
        display.len() as u64 + definition.map_or(0, |target| target.retained_bytes(spelling))
    }
}

/// The editor definition target of a resolved function callee: the file it is declared
/// in, its declared-name span (the selection range), and its header-through-body range.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DefinitionTarget {
    file: FileRef,
    name_span: FactSpan,
    decl_range: FactSpan,
}

impl DefinitionTarget {
    /// The target of a callee resolved to a declaration in `file`.
    pub(crate) fn new(file: FileRef, name_span: SourceSpan, decl_range: SourceSpan) -> Self {
        Self {
            file,
            name_span: FactSpan::of(name_span),
            decl_range: FactSpan::of(decl_range),
        }
    }

    /// The logical byte charge of one retained target: the spelling of the file it names.
    /// Its spans are fixed-size and charged by the count bound. The destructure is
    /// exhaustive for the reason given at [`HoverFact::retained_bytes`].
    fn retained_bytes(self, spelling: impl FnOnce(FileRef) -> u64) -> u64 {
        let DefinitionTarget {
            file,
            name_span: _,
            decl_range: _,
        } = self;
        spelling(file)
    }
}

/// Which typed ceiling the analysis fact ledger crossed. Maps exhaustively to
/// [`AnalysisResourceLimit::SnapshotFactCount`] / [`AnalysisResourceLimit::SnapshotFactBytes`]
/// at the failure boundary.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AnalysisFactLimit {
    Count { limit: u64 },
    Bytes { limit: u64 },
}

/// The complete retained fact set of one snapshot, sealed by the ledger's single
/// `finish`. Every collection is a boxed slice, so the amortized growth capacity the
/// ledger used while collecting is not retained.
#[derive(Default)]
pub(crate) struct RetainedFacts {
    /// Private to this module because [`HoverFact`] is: the type a producer must not be
    /// able to name is not reachable through this field either.
    hover_facts: Box<[HoverFact]>,
    pub(crate) broken_files: Box<[FileRef]>,
    dependency_gaps: Box<[(FileRef, FactSpan)]>,
    document_symbols: Box<[(FileRef, Box<[DeclSymbol]>)]>,
}

/// The finished terminal of one fact ledger: the complete retained set, or the typed
/// ceiling that discarded it.
///
/// A Limited terminal carries the ceiling and nothing else. The ledger's saturated count
/// and byte totals stay strictly internal: they exist so a ledger that has already
/// crossed keeps composing later input without unbounded growth, and publishing one
/// would be the fabricated total the typed limits prevent.
pub(crate) enum BoundedAnalysisFacts {
    Complete(RetainedFacts),
    Limited { limit: AnalysisFactLimit },
}

impl BoundedAnalysisFacts {
    /// Test support: the complete retained set, or a panic on a limited terminal.
    #[cfg(test)]
    #[track_caller]
    pub(crate) fn expect_complete(&self) -> &RetainedFacts {
        match self {
            BoundedAnalysisFacts::Complete(facts) => facts,
            BoundedAnalysisFacts::Limited { limit } => {
                panic!("expected a complete fact terminal, got {limit:?}")
            }
        }
    }
}

/// A selectively-queried editor fact. It is `Present`, legitimately `Absent`, or
/// `Unavailable` because a syntax or dependency invalidity prevents its computation. An
/// unknown file or an out-of-range offset is not absence — it is a typed [`QueryError`],
/// distinct from every `Fact` outcome.
pub enum Fact<T> {
    /// The fact is computed and present.
    Present(T),
    /// Every owner the fact reads is available, and there is no fact at the position.
    Absent,
    /// The fact cannot be computed because a required owner is invalid.
    Unavailable(Unavailability),
}

/// Why a fact could not be computed at a position whose file and offset are valid.
pub enum Unavailability {
    /// The position lies in a module that did not parse.
    Syntax,
    /// The fact reads a project-global owner contributed by a module that did not
    /// parse, so the owner is incomplete.
    Dependency,
    /// The fact crossed a fixed per-file bound, so it was never retained. No truncated
    /// value is ever published in its place, and no other fact — in this file or any
    /// other — is affected.
    Bounded,
}

/// Why a hover or definition query could not be resolved to a position at all. Distinct
/// from a `Fact` outcome: the coordinate itself is not a valid position in the snapshot's
/// input.
pub enum QueryError {
    /// The file is not one of the snapshot's analyzed input files.
    UnknownFile,
    /// The byte offset lies outside the file's source bytes.
    OffsetOutOfRange,
}

/// The hover fact at a source position: the compiler's canonical display of a local or
/// parameter's value type. It carries no effects, demand, or durable-anchor spelling.
pub struct Hover {
    display: String,
}

impl Hover {
    /// The canonical type display.
    pub fn display(&self) -> &str {
        &self.display
    }
}

/// The declaration kind of a [`DeclSymbol`], mirroring the parser's `Declaration`
/// variants plus the nested `EnumMember`. Closed and exhaustively matchable so a
/// consumer maps each kind to its editor symbol category without a wildcard, and a new
/// declaration variant forces a decision here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeclKind {
    /// A transparent `alias` type declaration.
    Alias,
    /// A nominal `type` declaration.
    Nominal,
    /// A module-private `const` declaration.
    Const,
    /// A durable `resource` declaration.
    Resource,
    /// A `struct` value-type declaration.
    Struct,
    /// A `store` saved-root declaration.
    Store,
    /// A `fn` function declaration.
    Function,
    /// An `enum` declaration.
    Enum,
    /// A `test` declaration.
    Test,
    /// One member of an enum, nested under its enum (recursively under a `category`).
    EnumMember,
}

/// One node of a module file's declaration hierarchy: a declared name, its kind, the
/// span of its declared name (the selection range), the full header-through-body
/// declaration range, and its nested member children. Children are non-empty only for an
/// enum and its nested `category` members; every other declaration is a leaf on this
/// floor. A pure projection of the parsed AST — it carries no resolved type, effect, or
/// durable-anchor spelling.
pub struct DeclSymbol {
    name: Box<str>,
    kind: DeclKind,
    name_span: FactSpan,
    full_range: FactSpan,
    children: Box<[DeclSymbol]>,
}

impl DeclSymbol {
    /// The declared name spelling.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The declaration kind.
    pub fn kind(&self) -> DeclKind {
        self.kind
    }

    /// The span of the declared name — the selection range. For a declaration whose AST
    /// carries no separate name span, this is the full declaration range.
    pub fn name_span(&self) -> SourceSpan {
        self.name_span.source()
    }

    /// The full header-through-body declaration range.
    pub fn full_range(&self) -> SourceSpan {
        self.full_range.source()
    }

    /// The nested member children, in source order.
    pub fn children(&self) -> &[DeclSymbol] {
        &self.children
    }

    /// This node's retained byte footprint: its name spelling. Spans and the kind are
    /// fixed-size and charged by the count bound; children are summed separately, each
    /// charging one count of its own. The destructure is exhaustive for the reason given
    /// at [`HoverFact::retained_bytes`].
    fn retained_bytes(&self) -> u64 {
        let DeclSymbol {
            name,
            kind: _,
            name_span: _,
            full_range: _,
            children: _,
        } = self;
        name.len() as u64
    }
}

/// The total number of symbol nodes in a projected outline, counting nested members.
fn symbol_count(symbols: &[DeclSymbol]) -> u64 {
    symbols
        .iter()
        .map(|symbol| 1 + symbol_count(&symbol.children))
        .sum()
}

/// The total retained byte footprint of a projected outline, counting nested members.
fn symbol_bytes(symbols: &[DeclSymbol]) -> u64 {
    symbols
        .iter()
        .map(|symbol| symbol.retained_bytes() + symbol_bytes(&symbol.children))
        .sum()
}

/// A projection exhausted a per-file declaration-hierarchy bound. Which of
/// [`MAX_DOCUMENT_SYMBOLS_PER_FILE`] and [`MAX_SYMBOL_DEPTH`] is not carried: either way
/// that file's outline is unavailable and nothing partial is retained for it.
pub(crate) struct SymbolBoundExceeded;

/// Project one module file's declarations into its declaration-hierarchy outline, or the
/// first per-file bound the outline would exceed. A pure projection over existing name
/// spans and declaration ranges: it reclassifies nothing.
pub(crate) fn project_document_symbols(
    declarations: &[Declaration],
) -> Result<Box<[DeclSymbol]>, SymbolBoundExceeded> {
    let mut builder = SymbolProjection { count: 0 };
    declarations
        .iter()
        .map(|declaration| builder.declaration(declaration, 1))
        .collect()
}

/// The bounded projection walk. It carries the running per-file node count and enforces
/// the count and depth bounds as it descends, so no outline is materialized past either
/// bound.
struct SymbolProjection {
    count: u64,
}

impl SymbolProjection {
    /// Admit one more node at `depth`, enforcing both per-file bounds before it is built.
    fn admit(&mut self, depth: u16) -> Result<(), SymbolBoundExceeded> {
        if depth > MAX_SYMBOL_DEPTH {
            return Err(SymbolBoundExceeded);
        }
        self.count += 1;
        if self.count > MAX_DOCUMENT_SYMBOLS_PER_FILE {
            return Err(SymbolBoundExceeded);
        }
        Ok(())
    }

    fn declaration(
        &mut self,
        declaration: &Declaration,
        depth: u16,
    ) -> Result<DeclSymbol, SymbolBoundExceeded> {
        self.admit(depth)?;
        let leaf = |name: &str, kind: DeclKind, name_span: SourceSpan, full_range: SourceSpan| {
            DeclSymbol {
                name: name.into(),
                kind,
                name_span: FactSpan::of(name_span),
                full_range: FactSpan::of(full_range),
                children: Box::default(),
            }
        };
        let symbol = match declaration {
            Declaration::Alias(alias) => {
                leaf(&alias.name, DeclKind::Alias, alias.name_span, alias.span)
            }
            Declaration::Nominal(nominal) => leaf(
                &nominal.name,
                DeclKind::Nominal,
                nominal.name_span,
                nominal.span,
            ),
            // A `const` declaration carries no separate name span in the AST, so its
            // selection range is its full declaration range.
            Declaration::Const(konst) => leaf(&konst.name, DeclKind::Const, konst.span, konst.span),
            Declaration::Resource(resource) => leaf(
                &resource.name,
                DeclKind::Resource,
                resource.name_span,
                resource.span,
            ),
            Declaration::Struct(item) => {
                leaf(&item.name, DeclKind::Struct, item.name_span, item.span)
            }
            // A store's declared name is its saved-root spelling; its name span covers
            // the `^root` sigiled root.
            Declaration::Store(store) => leaf(
                &store.root.root,
                DeclKind::Store,
                store.root.span,
                store.span,
            ),
            Declaration::Function(function) => leaf(
                &function.name,
                DeclKind::Function,
                function.name_span,
                function.span,
            ),
            Declaration::Test(test) => leaf(&test.name, DeclKind::Test, test.name_span, test.span),
            Declaration::Enum(item) => {
                let children = self.members(&item.members, depth + 1)?;
                DeclSymbol {
                    name: item.name.as_str().into(),
                    kind: DeclKind::Enum,
                    name_span: FactSpan::of(item.name_span),
                    full_range: FactSpan::of(item.span),
                    children,
                }
            }
        };
        Ok(symbol)
    }

    fn members(
        &mut self,
        members: &[EnumMember],
        depth: u16,
    ) -> Result<Box<[DeclSymbol]>, SymbolBoundExceeded> {
        members
            .iter()
            .map(|member| self.member(member, depth))
            .collect()
    }

    fn member(
        &mut self,
        member: &EnumMember,
        depth: u16,
    ) -> Result<DeclSymbol, SymbolBoundExceeded> {
        self.admit(depth)?;
        let children = self.members(&member.members, depth + 1)?;
        Ok(DeclSymbol {
            name: member.name.as_str().into(),
            kind: DeclKind::EnumMember,
            name_span: FactSpan::of(member.name_span),
            full_range: FactSpan::of(member.span),
            children,
        })
    }
}

/// The closed set of completion position classes, derived purely positionally from the
/// checker's resolution model over the queried file's parse — never from the trigger
/// character, document text, or a token scan. Each class fixes which namespace
/// [`AnalysisSnapshot::completions`] enumerates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PositionClass {
    /// An identifier (or partial identifier) in expression position: locals and
    /// parameters in scope before the position, module functions, consts, built-ins,
    /// imported module names, and enum type names.
    ExpressionName,
    /// After `.`/`?.` on a receiver: the base type's declared fields when the base
    /// resolves to a struct type, else an empty candidate set.
    Member,
    /// After `::` on a resolved enum path: that enum node's immediate members, categories
    /// marked non-selectable.
    EnumPath,
    /// A type-annotation position: named types, generic templates, built-in type names,
    /// and in-scope type parameters.
    TypeAnnotation,
}

/// The closed kind of one completion candidate, so a consumer maps each to its editor
/// symbol category without a wildcard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CandidateKind {
    /// A module function (monomorphic or a generic template).
    Function,
    /// A value-level built-in (`some`, `trim`, `List`, ...).
    Builtin,
    /// A local binding in scope before the position.
    Local,
    /// A function parameter.
    Param,
    /// A module-private const.
    Const,
    /// A declared struct field.
    Field,
    /// An enum member; `selectable` is false for a `category` member.
    EnumMember { selectable: bool },
    /// A named type, alias, generic template, or built-in type name.
    Type,
    /// An in-scope generic type parameter.
    TypeParam,
    /// An imported module name.
    Module,
}

/// One completion candidate: the declared spelling to insert, its kind, and a canonical
/// detail display. `detail` renders the candidate's declared type or signature spelling,
/// and is empty when the declaration carries no annotation.
pub struct Candidate {
    label: String,
    kind: CandidateKind,
    detail: String,
}

impl Candidate {
    /// The declared spelling to insert.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The candidate kind.
    pub fn kind(&self) -> CandidateKind {
        self.kind
    }

    /// The canonical detail display (declared type or signature spelling), possibly empty.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// The completion fact at a position: the position class and its complete in-scope
/// candidate namespace.
pub struct Completions {
    class: PositionClass,
    candidates: Vec<Candidate>,
}

impl Completions {
    /// The position class.
    pub fn class(&self) -> PositionClass {
        self.class
    }

    /// The complete in-scope candidate set for the class, in a stable enumeration order.
    pub fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }
}

/// The outcome of a completion query. A `Ready` outcome carries the ordinary [`Fact`] —
/// present classification, legitimate absence, or an unavailable owner. A `Refused`
/// outcome is an unretained query-local resource refusal, never a truncated prefix. An
/// unknown file or an out-of-range offset is a typed [`QueryError`], distinct from both.
pub enum CompletionOutcome {
    /// A computed completion fact.
    Ready(Fact<Completions>),
    /// The in-scope candidate set exceeded a per-query bound
    /// ([`AnalysisResourceLimit::CompletionCandidateCount`] or
    /// [`AnalysisResourceLimit::CompletionRenderBytes`]); a query-local refusal.
    Refused(AnalysisResourceLimit),
}

/// One parameter piece of an active call's signature: the declared spelling of a single
/// parameter (`name: Type`). Each piece composes the signature display exactly, so a
/// consumer that does locate pieces in the display finds an exact match.
pub struct ParamPiece {
    label: String,
}

impl ParamPiece {
    /// The declared spelling of this parameter (`name: Type`).
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// The active-call fact at a position: the innermost enclosing call's callee signature
/// display, its parameter pieces in declaration order, and the active argument index the
/// offset sits at. `active` is `None` when the callee declares no parameters; otherwise it
/// is the slot the cursor occupies, which may sit past the last parameter when more
/// arguments than parameters are present.
pub struct ActiveCall {
    signature: String,
    params: Vec<ParamPiece>,
    active: Option<u16>,
}

impl ActiveCall {
    /// The canonical callee signature display (`fn name(pieces): ret`, a generic callee
    /// carrying its template `<...>` parameters).
    pub fn signature(&self) -> &str {
        &self.signature
    }

    /// The parameter pieces in declaration order.
    pub fn params(&self) -> &[ParamPiece] {
        &self.params
    }

    /// The active argument index, or `None` when the callee declares no parameters.
    pub fn active(&self) -> Option<u16> {
        self.active
    }
}

/// The outcome of an active-call query. A `Ready` outcome carries the ordinary [`Fact`] —
/// a present active-call fact, a legitimate absence, or an unavailable owner. A `Refused`
/// outcome is an unretained query-local resource refusal, never a truncated display. An
/// unknown file or an out-of-range offset is a typed [`QueryError`], distinct from both.
pub enum ActiveCallOutcome {
    /// A computed active-call fact.
    Ready(Fact<ActiveCall>),
    /// The rendered signature-and-parameter display exceeded
    /// [`AnalysisResourceLimit::ActiveCallRenderBytes`]; a query-local refusal.
    Refused(AnalysisResourceLimit),
}
