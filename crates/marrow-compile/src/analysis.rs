//! The editor analysis fact floor: one immutable, revisioned [`AnalysisSnapshot`] per
//! exact project input.
//!
//! A caller hands [`analyze`] the exact [`ProjectInput`] it wants analyzed and a
//! [`InputRevision`] it assigns. The revision labels which input a result belongs to;
//! the floor echoes it and never treats it as content identity or an ordering key. The
//! snapshot enumerates the complete, resilient diagnostic set — every stage's
//! diagnostics over every module, so an independent valid component keeps its
//! diagnostics even when a sibling fails to parse — and holds the caller's same
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
use crate::{CompileInvariant, CompileResourceLimit, SourceDiagnostic};

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
/// law-9 allocation guard, evidence-widenable; four times the CRES01 diagnostic-byte
/// ceiling gives headroom for nested-generic type displays without unbounded retention.
pub const MAX_SNAPSHOT_FACT_BYTES: u64 = 4 * 1024 * 1024;

/// The largest number of in-scope completion candidates one query assembles before it is
/// refused as a query-local [`AnalysisResourceLimit::CompletionCandidateCount`]. The
/// candidate set is the complete in-scope namespace for the position class — never
/// prefix-filtered, ranked, or truncated — so an over-cap namespace is a typed refusal,
/// never a truncated prefix. Query-local; candidate sets are never retained per position.
pub const MAX_COMPLETION_CANDIDATES: u64 = 512;

/// The largest total rendered-candidate byte footprint one completion query assembles
/// (each candidate's label plus its detail) before it is refused as a query-local
/// [`AnalysisResourceLimit::CompletionRenderBytes`]. A query-local expansion guard, not a
/// retained snapshot bound.
pub const MAX_COMPLETION_RENDER_BYTES: u64 = 256 * 1024;

/// The largest total rendered byte footprint one active-call query assembles (the callee
/// signature display plus every parameter piece) before it is refused as a query-local
/// [`AnalysisResourceLimit::ActiveCallRenderBytes`]. The callee's parameter arity is
/// already bounded by the compiler's declaration bounds; this is a query-local expansion
/// guard on the rendered display, not a retained snapshot bound.
pub const MAX_ACTIVE_CALL_RENDER_BYTES: u64 = 64 * 1024;

/// The largest checked whole-document format output one query returns before it is
/// refused as a query-local outcome (never retained). The formatter's input is already
/// bounded by the pure owner's per-file admission, so this is an expansion guard, not a
/// second input bound.
pub const MAX_FORMAT_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;

/// The largest number of declaration-hierarchy symbols one module file admits before its
/// snapshot is transactionally refused as a [`AnalysisResourceLimit::DocumentSymbolCount`].
/// Every projected node — each top-level declaration and each nested enum member — counts
/// once. No partial or truncated outline is retained.
pub const MAX_DOCUMENT_SYMBOLS_PER_FILE: u64 = 4_096;

/// The largest declaration-hierarchy nesting depth one module file admits before its
/// snapshot is refused as a [`AnalysisResourceLimit::DocumentSymbolDepth`]. Top-level
/// declarations sit at depth one; enum members deepen the tree by one level each. The
/// parser admits far deeper enum-member nesting, so this analysis bound is reachable and
/// fails a pathological outline closed rather than recursing without limit.
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
/// The compaction is load-bearing, not cosmetic: a [`FileIdentity`] is an owned
/// spelling of up to 4096 bytes, and one clone per retained fact is up to 256 MiB of
/// retention at the pinned fact-count ceiling. Its *logical* charge is unchanged —
/// [`AnalysisFactCollector`] still charges a definition target's file spelling and a
/// document-symbol module's owner spelling exactly as before.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
/// offset is far inside `u32`. The equality of those two domains is pinned by a test,
/// exactly as the diagnostic owner pins its ceiling against the syntax owner's.
///
/// Retained spans dominate the snapshot's structural footprint — four of them per hover
/// fact and its target, four per document-symbol node — so carrying the source owner's
/// 64-bit offsets in retained state would cost megabytes to represent a megabyte's
/// worth of positions.
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
        // always false and every fact at it silently absent. State the domain instead:
        // the admission ceiling is inside it (`the_admission_ceiling_fits_the_fact_
        // coordinate_domain`), so a widened ceiling is a failing debug assertion here
        // rather than facts that quietly stop resolving.
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

/// A fixed analysis resource bound that produced no snapshot. It wraps CRES01's shipped
/// [`CompileResourceLimit`] verbatim for a compile-side aggregate bound, and names the
/// snapshot fact bounds directly. Closed and exhaustively matchable.
pub enum AnalysisResourceLimit {
    /// A compile-side aggregate bound (an image count/byte ceiling, or the CRES01
    /// diagnostic count/byte ceiling on the complete analysis diagnostic set).
    Compile(CompileResourceLimit),
    /// The retained fact count exceeded [`MAX_SNAPSHOT_FACT_COUNT`].
    SnapshotFactCount { limit: u64 },
    /// The retained fact byte footprint exceeded [`MAX_SNAPSHOT_FACT_BYTES`].
    SnapshotFactBytes { limit: u64 },
    /// One module file's declaration-hierarchy symbol count exceeded
    /// [`MAX_DOCUMENT_SYMBOLS_PER_FILE`].
    DocumentSymbolCount { limit: u64 },
    /// One module file's declaration-hierarchy nesting depth exceeded
    /// [`MAX_SYMBOL_DEPTH`].
    DocumentSymbolDepth { limit: u16 },
    /// One completion query's in-scope candidate set exceeded
    /// [`MAX_COMPLETION_CANDIDATES`]. A query-local refusal (never a truncated prefix),
    /// not a retained snapshot bound.
    CompletionCandidateCount { limit: u64 },
    /// One completion query's rendered candidate byte footprint exceeded
    /// [`MAX_COMPLETION_RENDER_BYTES`]. A query-local refusal, not a retained snapshot
    /// bound.
    CompletionRenderBytes { limit: u64 },
    /// One active-call query's rendered signature-and-parameter byte footprint exceeded
    /// [`MAX_ACTIVE_CALL_RENDER_BYTES`]. A query-local refusal, not a retained snapshot
    /// bound.
    ActiveCallRenderBytes { limit: u64 },
}

impl AnalysisResourceLimit {
    /// The sentence fragment a person reads for the exhausted bound, lowercase and
    /// unpunctuated. A compile-side bound answers in
    /// [`ResourceLimitKind`](crate::ResourceLimitKind)'s own words, so one bound reads
    /// the same whichever owner reports it. No Rust variant name reaches a reader.
    pub fn description(&self) -> &'static str {
        match self {
            AnalysisResourceLimit::Compile(limit) => limit.kind().description(),
            AnalysisResourceLimit::SnapshotFactCount { .. } => "the analysis fact table is full",
            AnalysisResourceLimit::SnapshotFactBytes { .. } => {
                "the analysis facts hold too much text to retain"
            }
            AnalysisResourceLimit::DocumentSymbolCount { .. } => {
                "one file declares too many symbols"
            }
            AnalysisResourceLimit::DocumentSymbolDepth { .. } => {
                "one file's declarations are nested too deeply"
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
/// Every retained collection is a boxed slice: a snapshot is immutable, so the
/// growth capacity an amortized `Vec` carries is not part of its retained state and
/// is not charged to the caller who holds one.
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
    /// declaration order. A file that did not parse has no entry — it is in
    /// `broken_files` — and a `document_symbols` query for it is
    /// [`Unavailability::Syntax`], not an absent tree.
    document_symbols: Box<[(FileRef, Box<[DeclSymbol]>)]>,
    /// Files whose outline crossed [`MAX_DOCUMENT_SYMBOLS_PER_FILE`] or
    /// [`MAX_SYMBOL_DEPTH`]. Nothing is retained for such a file, so its
    /// `document_symbols` is [`Unavailability::Bounded`]; every other query for it, and
    /// every query for every other file, is unaffected.
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
    /// [`FileRef`] and its source bytes, or a typed query error when the file is not
    /// one of the snapshot's analyzed inputs. Every fact query and every retained
    /// span resolves through here, so a fact can only ever index bytes this snapshot
    /// holds.
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
    /// Definition covers source-defined function callees, including a call inside a generic
    /// template body (collected once at the template); a generic call targets its source
    /// template. Local/parameter, type, import, and field definitions are not covered.
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
    /// cleanly-parsed file with no declarations is a truthful `Present` empty outline.
    ///
    /// This is a pure projection: it reclassifies nothing and reads no resolved semantic
    /// identity. The outline is retained per snapshot and bounded per file by
    /// [`MAX_DOCUMENT_SYMBOLS_PER_FILE`] and [`MAX_SYMBOL_DEPTH`] at snapshot admission.
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
            // A validated input that is neither broken nor retained did not parse cleanly;
            // the honest outcome is the same syntax-unavailable verdict, never a fabricated
            // empty tree.
            None => Ok(Fact::Unavailable(Unavailability::Syntax)),
        }
    }

    /// The completion classification and candidate namespace at a byte offset in a file.
    ///
    /// The position class is derived purely positionally from the checker's resolution
    /// model over a parse of this file's own retained bytes — never from the trigger
    /// character, document text, or a token scan. The candidate set is the complete
    /// in-scope namespace for the class: locals and parameters in scope before the
    /// offset, module functions, consts, built-ins, imported module names, and enum type
    /// names for an expression name; the base type's declared fields after `.`/`?.`; an
    /// enum's immediate members after `::`; named types, generic templates, built-in type
    /// names, and in-scope type parameters in a type annotation.
    ///
    /// The set is never prefix-filtered, ranked, or truncated: an over-cap namespace is a
    /// query-local [`CompletionOutcome::Refused`], never a truncated prefix. The parse
    /// and the re-resolution over it are per query and transient — no parse tree and no
    /// per-position candidate set is retained.
    ///
    /// An unknown file or an out-of-range offset is a typed [`QueryError`]. A file that
    /// produced no parse tree (a non-UTF-8 file) is [`Unavailability::Syntax`]. A broken
    /// file still classifies: a position over a recovered incomplete form (`base.`,
    /// `Enum::`) yields its class and candidates even though the file has parse errors.
    /// A position with no class (a literal, a comment, whitespace outside any recovered
    /// node) is `Absent`.
    ///
    /// The traversal is strictly read-only: it never drives the compile-path lowerer or
    /// resolver, so a partial or malformed base yields an `Absent`/empty classification and
    /// leaks no diagnostic into the snapshot.
    pub fn completions(
        &self,
        file: &FileIdentity,
        offset: usize,
    ) -> Result<CompletionOutcome, QueryError> {
        let (_, source) = self.locate(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        let Some(tree) = query_local_parse(source) else {
            // A validated input file that cannot be decoded never produced a tree. The
            // honest verdict is syntax-unavailable, never a fabricated empty set.
            return Ok(CompletionOutcome::Ready(Fact::Unavailable(
                Unavailability::Syntax,
            )));
        };
        Ok(completion::resolve(&tree, offset as u32))
    }

    /// The active-call fact at a byte offset: the innermost enclosing call's callee
    /// signature, its parameter pieces, and the active argument index the offset sits at.
    ///
    /// The enclosing call and active index are derived purely positionally over a parse
    /// of this file's own retained bytes — never from the trigger character or a
    /// document-text scan. The callee resolves to a same-module function or generic
    /// template declared in the file, and a generic callee presents its source template
    /// signature. The parameter pieces are separately rendered from the declared
    /// spellings so no consumer substring-searches the signature display, and each piece
    /// composes the signature so a consumer can mark the active one.
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
        let Some(tree) = query_local_parse(source) else {
            // A validated input file that cannot be decoded never produced a tree. The
            // honest verdict is syntax-unavailable, never a fabricated absence.
            return Ok(ActiveCallOutcome::Ready(Fact::Unavailable(
                Unavailability::Syntax,
            )));
        };
        Ok(active_call::resolve(&tree, source, offset as u32))
    }
}

/// Parse exactly one already-admitted file's already-retained bytes for one query.
///
/// The tree is transient: it is never retained, never enters a collector, and
/// contributes no diagnostic. `broken_files` stays the independent record of
/// parseability — no query infers parseability from this parse, and a recovered
/// broken file still classifies positions over its recovered forms, exactly as it did
/// when the tree was retained. Parsing is a pure function of the source bytes, so a
/// query's outcome does not depend on when the parse ran.
///
/// Its peak is charged before it is incurred, and by an owner that runs before any file
/// is parsed: [`crate::MAX_PARSED_FILE_BYTES`] is the longest file drive admission
/// accepts, and it is derived from [`crate::MAX_QUERY_PARSE_TRANSIENT_BYTES`] and the
/// rate `marrow-syntax` publishes for the representation it builds. Every file a
/// snapshot holds therefore has an accounted parse charge under that ceiling, and this
/// needs no refusal arm of its own — an arm here would be unreachable, and an
/// unreachable refusal is a claim no test can keep honest.
fn query_local_parse(source: &[u8]) -> Option<marrow_syntax::SourceFile> {
    let source = std::str::from_utf8(source).ok()?;
    Some(marrow_syntax::parse_source(source).file)
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
        Analyzed::Diagnostics(diagnostics) => diagnostics,
    };
    // The fact ledger admitted every fact against its ceilings at the push, so the
    // sealed terminal is either the complete retained set or the typed limit that
    // discarded it. Project the limit through one exhaustive translation, mirroring the
    // diagnostic owner's failure boundary; no partial fact set is ever published.
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
/// bulk. That makes the staging defect this row deleted unrepresentable rather than
/// merely scanned for. Producers reach the ledger through [`FactSink::hover`], which
/// takes the parts and admits at the push.
struct HoverFact {
    file: FileRef,
    span: FactSpan,
    display: Box<str>,
    /// The definition target when this fact is a resolved function callee; `None` for a
    /// local or parameter use.
    ///
    /// Carried inline. Every coordinate in it is compact, so an inlined target costs the
    /// fact struct less than a second retained table plus a reference into it would cost
    /// the accounted worst case (`the_accounted_footprint_closes_under_the_exported_term`
    /// derives both), and the snapshot keeps one retained fact family instead of two.
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
    /// Its spans are fixed-size and charged by the count bound.
    ///
    /// The destructure is exhaustive so a new heap-owning field on this retained type is
    /// a build error here rather than retention the exported term never saw.
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
/// A Limited terminal carries the ceiling and nothing else. The ledger's saturated
/// count and byte totals stay strictly internal: they exist so a ledger that has
/// already crossed keeps composing later input without unbounded growth, and
/// publishing one would be exactly the fabricated total the typed limits prevent.
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

/// The one live private analysis-fact owner.
///
/// It is the structural sibling of the diagnostic collector: every fact is admitted
/// against the typed count and byte ceilings **at the push**, so no fact set larger
/// than a public snapshot bound is ever materialized. Crossing a ceiling discards the
/// whole payload — the incoming fact and every already-admitted one — because a
/// crossing refuses the whole snapshot; there is no partial publication to unwind and
/// therefore no transaction, epoch, or receipt protocol.
///
/// Count retains precedence over bytes, and a `Bytes` limit strengthens to `Count`
/// once the composed count crosses; `Count` never weakens.
pub(crate) struct AnalysisFactCollector {
    /// The spelling length of each admitted module, by [`FileRef`]. The ledger owns
    /// the logical byte charges, which are stated over file *spellings* and stay
    /// exact even though the compact representation no longer stores one per fact.
    file_bytes: Vec<u32>,
    state: FactState,
}

enum FactState {
    Retaining {
        /// The running admitted count. Unlike the diagnostic owner's `rows.len()`,
        /// this total spans three fact families and counts every nested
        /// document-symbol node, so recomputing it per admission would be quadratic.
        count: u64,
        bytes: u64,
        facts: RetainingFacts,
    },
    Limited {
        count: u64,
        bytes: u64,
        limit: AnalysisFactLimit,
    },
}

/// The growable form of the retained set, before `finish` seals it.
#[derive(Default)]
struct RetainingFacts {
    hover_facts: Vec<HoverFact>,
    broken_files: Vec<FileRef>,
    dependency_gaps: Vec<(FileRef, FactSpan)>,
    document_symbols: Vec<(FileRef, Box<[DeclSymbol]>)>,
}

impl RetainingFacts {
    fn seal(self) -> RetainedFacts {
        RetainedFacts {
            hover_facts: self.hover_facts.into_boxed_slice(),
            broken_files: self.broken_files.into_boxed_slice(),
            dependency_gaps: self.dependency_gaps.into_boxed_slice(),
            document_symbols: self.document_symbols.into_boxed_slice(),
        }
    }
}

impl AnalysisFactCollector {
    /// A fresh ledger over `project`'s admitted modules. Drive admission runs before
    /// this, so the module count is already inside the [`FileRef`] domain.
    pub(crate) fn new(project: &ProjectInput) -> Self {
        Self {
            file_bytes: project
                .modules()
                .iter()
                .map(|module| module.identity().as_str().len() as u32)
                .collect(),
            state: FactState::Retaining {
                count: 0,
                bytes: 0,
                facts: RetainingFacts::default(),
            },
        }
    }

    /// Whether a ceiling has already been crossed. The drive stops rendering fact
    /// displays once this is true: the whole snapshot is already refused, so every
    /// further render is waste. This is an allocation bound, not a protocol.
    pub(crate) fn is_limited(&self) -> bool {
        matches!(self.state, FactState::Limited { .. })
    }

    /// A scoped borrow for one body's lowering, so a producer writes facts through the
    /// ledger rather than into a vector of its own.
    pub(crate) fn sink(&mut self, file: FileRef) -> FactSink<'_> {
        FactSink::Retaining { ledger: self, file }
    }

    /// The logical byte charge of one file's spelling. Every coordinate the drive mints
    /// names a module of the project this ledger was built over, so the lookup is total;
    /// an absent one would under-charge silently rather than refuse.
    fn spelling_bytes(&self, file: FileRef) -> u64 {
        debug_assert!(
            file.index() < self.file_bytes.len(),
            "a coordinate names a module of this ledger's own project"
        );
        self.file_bytes.get(file.index()).copied().unwrap_or(0) as u64
    }

    /// Retain one hover fact. Charges one count and the fact's own logical byte charge —
    /// unchanged by the compact representation, which no longer stores a file spelling
    /// per fact.
    pub(crate) fn admit_hover(
        &mut self,
        file: FileRef,
        span: SourceSpan,
        display: Box<str>,
        definition: Option<DefinitionTarget>,
    ) {
        let fact = HoverFact {
            file,
            span: FactSpan::of(span),
            display,
            definition,
        };
        let bytes = fact.retained_bytes(|at| self.spelling_bytes(at));
        self.admit(1, bytes, move |facts| facts.hover_facts.push(fact));
    }

    /// Retain one dependency gap. It carries only fixed-size references and a span, so
    /// the count bound charges it and it charges no bytes.
    pub(crate) fn admit_gap(&mut self, file: FileRef, span: SourceSpan) {
        self.admit(1, 0, |facts| {
            facts.dependency_gaps.push((file, FactSpan::of(span)));
        });
    }

    /// Retain one module's declaration-hierarchy outline. Charges one count per
    /// projected node, counting nested members, and its owner file spelling once plus
    /// every retained symbol-name spelling.
    pub(crate) fn admit_symbols(&mut self, file: FileRef, symbols: Box<[DeclSymbol]>) {
        let count = symbol_count(&symbols);
        let bytes = self.spelling_bytes(file) + symbol_bytes(&symbols);
        self.admit(count, bytes, |facts| {
            facts.document_symbols.push((file, symbols));
        });
    }

    /// Record that a module did not parse. Broken-module status is not a public fact
    /// row: it is one coordinate per module, bounded by the same 4096-file admission
    /// limit that bounds the coordinate domain, so it charges neither ceiling.
    pub(crate) fn admit_broken(&mut self, file: FileRef) {
        if let FactState::Retaining { facts, .. } = &mut self.state {
            facts.broken_files.push(file);
        }
    }

    /// Seal this ledger into its terminal. Total: every state has a terminal.
    pub(crate) fn finish(self) -> BoundedAnalysisFacts {
        match self.state {
            FactState::Retaining { facts, .. } => BoundedAnalysisFacts::Complete(facts.seal()),
            FactState::Limited { limit, .. } => BoundedAnalysisFacts::Limited { limit },
        }
    }

    /// Admit one contribution against both ceilings before `retain` may allocate for
    /// it. Crossing discards the whole payload, including the admitted prefix, and
    /// Count wins a simultaneous crossing.
    fn admit(
        &mut self,
        added_count: u64,
        added_bytes: u64,
        retain: impl FnOnce(&mut RetainingFacts),
    ) {
        match &mut self.state {
            FactState::Retaining {
                count,
                bytes,
                facts,
            } => {
                let new_count = count.saturating_add(added_count);
                let new_bytes = bytes.saturating_add(added_bytes);
                if new_count > MAX_SNAPSHOT_FACT_COUNT {
                    self.state = limited_facts(
                        new_count,
                        new_bytes,
                        AnalysisFactLimit::Count {
                            limit: MAX_SNAPSHOT_FACT_COUNT,
                        },
                    );
                } else if new_bytes > MAX_SNAPSHOT_FACT_BYTES {
                    self.state = limited_facts(
                        new_count,
                        new_bytes,
                        AnalysisFactLimit::Bytes {
                            limit: MAX_SNAPSHOT_FACT_BYTES,
                        },
                    );
                } else {
                    *count = new_count;
                    *bytes = new_bytes;
                    retain(facts);
                }
            }
            FactState::Limited {
                count,
                bytes,
                limit,
            } => {
                *count = count
                    .saturating_add(added_count)
                    .min(MAX_SNAPSHOT_FACT_COUNT + 1);
                *bytes = bytes
                    .saturating_add(added_bytes)
                    .min(MAX_SNAPSHOT_FACT_BYTES + 1);
                if matches!(limit, AnalysisFactLimit::Bytes { .. })
                    && *count > MAX_SNAPSHOT_FACT_COUNT
                {
                    *limit = AnalysisFactLimit::Count {
                        limit: MAX_SNAPSHOT_FACT_COUNT,
                    };
                }
            }
        }
    }
}

/// The saturated Limited state: totals cap at ceiling plus one, so later input keeps
/// composing without unbounded growth. The whole retained payload is dropped here — it
/// never re-materializes.
fn limited_facts(count: u64, bytes: u64, limit: AnalysisFactLimit) -> FactState {
    FactState::Limited {
        count: count.min(MAX_SNAPSHOT_FACT_COUNT + 1),
        bytes: bytes.min(MAX_SNAPSHOT_FACT_BYTES + 1),
        limit,
    }
}

/// The scoped borrow one body's lowering writes its editor facts through.
///
/// A producer never holds a fact vector: every fact reaches the ledger's ceilings at the
/// push that produced it, so no single body can stage more facts than a whole snapshot
/// admits. A body whose facts duplicate an already-collected template's is given the
/// `Discarding` state rather than a scratch vector nobody reads.
pub(crate) enum FactSink<'a> {
    Retaining {
        ledger: &'a mut AnalysisFactCollector,
        file: FileRef,
    },
    /// This body's facts duplicate a template's, which were collected once at the
    /// template proof. Nothing is retained and nothing is allocated.
    Discarding,
}

impl FactSink<'_> {
    /// Admit one editor hover fact in this sink's file, at the push that produced it.
    /// The ledger charges it against both ceilings before it is retained, so the count a
    /// single body can hold live is bounded by the snapshot ceiling rather than by the
    /// body's length.
    pub(crate) fn hover(
        &mut self,
        span: SourceSpan,
        display: Box<str>,
        definition: Option<DefinitionTarget>,
    ) {
        if let FactSink::Retaining { ledger, file } = self {
            ledger.admit_hover(*file, span, display, definition);
        }
    }

    /// Retain one dependency gap in this sink's file. Gaps are written as they are
    /// discovered, so one survives even when the body it sits in fails to lower.
    pub(crate) fn gap(&mut self, span: SourceSpan) {
        if let FactSink::Retaining { ledger, file } = self {
            ledger.admit_gap(*file, span);
        }
    }

    /// Whether a fact written here would still be retained. A producer renders a fact
    /// display only inside this guard: a discarding sink keeps nothing, and once the
    /// ledger is Limited the whole snapshot is already refused, so both are waste.
    pub(crate) fn renders_facts(&self) -> bool {
        match self {
            FactSink::Retaining { ledger, .. } => !ledger.is_limited(),
            FactSink::Discarding => false,
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
    /// charging one count of its own.
    ///
    /// The destructure is exhaustive so a new heap-owning field on this retained type is
    /// a build error here rather than retention the exported term never saw.
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

/// Which per-file declaration-hierarchy bound a projection exhausted. Internal to the
/// projection; [`analyze`] maps it to the matching [`AnalysisResourceLimit`].
pub(crate) enum SymbolLimit {
    Count,
    Depth,
}

/// Project one module file's declarations into its declaration-hierarchy outline, or the
/// first per-file bound the outline would exceed. A pure projection over existing name
/// spans and declaration ranges: it reclassifies nothing.
pub(crate) fn project_document_symbols(
    declarations: &[Declaration],
) -> Result<Box<[DeclSymbol]>, SymbolLimit> {
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
    fn admit(&mut self, depth: u16) -> Result<(), SymbolLimit> {
        if depth > MAX_SYMBOL_DEPTH {
            return Err(SymbolLimit::Depth);
        }
        self.count += 1;
        if self.count > MAX_DOCUMENT_SYMBOLS_PER_FILE {
            return Err(SymbolLimit::Count);
        }
        Ok(())
    }

    fn declaration(
        &mut self,
        declaration: &Declaration,
        depth: u16,
    ) -> Result<DeclSymbol, SymbolLimit> {
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
    ) -> Result<Box<[DeclSymbol]>, SymbolLimit> {
        members
            .iter()
            .map(|member| self.member(member, depth))
            .collect()
    }

    fn member(&mut self, member: &EnumMember, depth: u16) -> Result<DeclSymbol, SymbolLimit> {
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
/// detail display. `detail` renders the declared type or signature spelling of the
/// candidate; it is empty when the declaration carries no annotation. The set a query
/// returns is the complete in-scope namespace — never prefix-filtered, ranked, or
/// truncated.
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

/// The outcome of a completion query. A `Ready` outcome carries the ordinary
/// [`Fact`] — present classification, legitimate absence, or an unavailable owner. A
/// `Refused` outcome is a query-local resource refusal (an over-cap candidate set), never
/// a truncated prefix; it is not retained. An unknown file or an out-of-range offset is a
/// typed [`QueryError`] distinct from every outcome here.
pub enum CompletionOutcome {
    /// A computed completion fact.
    Ready(Fact<Completions>),
    /// The in-scope candidate set exceeded a per-query bound
    /// ([`AnalysisResourceLimit::CompletionCandidateCount`] or
    /// [`AnalysisResourceLimit::CompletionRenderBytes`]); a query-local refusal.
    Refused(AnalysisResourceLimit),
}

/// One parameter piece of an active call's signature: the declared spelling of a single
/// parameter (`name: Type`). The pieces are rendered separately from the signature display
/// so a consumer marks the active parameter without substring-searching the display, and
/// each piece composes the signature so a consumer that does locate pieces in the display
/// finds an exact match.
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
/// outcome is a query-local resource refusal (an over-cap rendered display), never a
/// truncated display; it is not retained. An unknown file or an out-of-range offset is a
/// typed [`QueryError`] distinct from every outcome here.
pub enum ActiveCallOutcome {
    /// A computed active-call fact.
    Ready(Fact<ActiveCall>),
    /// The rendered signature-and-parameter display exceeded
    /// [`AnalysisResourceLimit::ActiveCallRenderBytes`]; a query-local refusal.
    Refused(AnalysisResourceLimit),
}

/// The per-query read-only completion re-resolution.
///
/// This is a distinct read-only pass over the query-local parse. It never drives the
/// compile-path lowerer or resolver — whose arms assume post-`has_errors` input and can
/// raise resolution invariants — so it runs safely on a broken file and leaks no
/// diagnostic. A partial or unresolvable base yields an empty candidate set; the position
/// class is derived purely positionally.
mod completion {
    use marrow_syntax::{
        Block, Declaration, EnumDecl, EnumMember, Expression, FunctionDecl, NameSegment, Recovery,
        ResourceMember, SourceFile, SourceSpan, Statement, TypeExpr,
    };

    use crate::lower::builtin_value_names;
    use crate::scalar::ScalarType;

    use super::{
        AnalysisResourceLimit, Candidate, CandidateKind, CompletionOutcome, Completions, Fact,
        MAX_COMPLETION_CANDIDATES, MAX_COMPLETION_RENDER_BYTES, PositionClass,
    };

    /// One in-scope binding: its spelling and, when annotated, its declared type node
    /// borrowed from that parse. The type node is the fail-soft type-probe: a
    /// bare struct-name annotation (a single-segment [`TypeExpr::Name`]) resolves to that
    /// struct's fields; any other type shape resolves to no fields.
    struct Binding<'a> {
        name: String,
        ty: Option<&'a TypeExpr>,
    }

    /// The lexical scope accumulated while descending to the offset: the enclosing
    /// declaration's generic type parameters, its parameters, and the locals introduced
    /// before the offset. A superset is never built — only bindings that precede the
    /// offset on the path to it are added.
    #[derive(Default)]
    struct Scope<'a> {
        type_params: Vec<String>,
        params: Vec<Binding<'a>>,
        locals: Vec<Binding<'a>>,
    }

    /// The positional classification of the offset, with the base receiver borrowed for a
    /// member or enum-path position.
    enum Located<'a> {
        ExprName,
        Member(&'a Expression),
        EnumPath(&'a Expression),
        TypeAnnotation,
    }

    /// Classify the offset over the queried file's tree and enumerate the class namespace.
    pub(super) fn resolve(file: &SourceFile, offset: u32) -> CompletionOutcome {
        let mut scope = Scope::default();
        let Some(located) = locate_file(file, offset, &mut scope) else {
            return CompletionOutcome::Ready(Fact::Absent);
        };
        let (class, candidates) = match located {
            Located::ExprName => (
                PositionClass::ExpressionName,
                expression_name_candidates(file, &scope),
            ),
            Located::Member(base) => (PositionClass::Member, member_candidates(file, &scope, base)),
            Located::EnumPath(base) => (PositionClass::EnumPath, enum_path_candidates(file, base)),
            Located::TypeAnnotation => (
                PositionClass::TypeAnnotation,
                type_annotation_candidates(file, &scope),
            ),
        };
        finish(class, candidates)
    }

    /// Apply the per-query candidate-count and render-byte caps, then package the fact. An
    /// over-cap namespace is a query-local refusal, never a truncated prefix.
    fn finish(class: PositionClass, candidates: Vec<Candidate>) -> CompletionOutcome {
        if candidates.len() as u64 > MAX_COMPLETION_CANDIDATES {
            return CompletionOutcome::Refused(AnalysisResourceLimit::CompletionCandidateCount {
                limit: MAX_COMPLETION_CANDIDATES,
            });
        }
        let bytes: u64 = candidates
            .iter()
            .map(|candidate| (candidate.label.len() + candidate.detail.len()) as u64)
            .sum();
        if bytes > MAX_COMPLETION_RENDER_BYTES {
            return CompletionOutcome::Refused(AnalysisResourceLimit::CompletionRenderBytes {
                limit: MAX_COMPLETION_RENDER_BYTES,
            });
        }
        CompletionOutcome::Ready(Fact::Present(Completions { class, candidates }))
    }

    pub(super) fn contains(span: SourceSpan, offset: u32) -> bool {
        span.start_byte as u32 <= offset && offset <= span.end_byte as u32
    }

    fn ends_before(span: SourceSpan, offset: u32) -> bool {
        (span.end_byte as u32) < offset
    }

    /// The byte extent of a declaration, including its body. A `fn`/`test` declaration's
    /// own `span` covers only the header through the opening brace; the body block is a
    /// separate span, so the extent unions the two. Every other declaration's `span`
    /// already covers its whole construct.
    pub(super) fn declaration_contains(declaration: &Declaration, offset: u32) -> bool {
        let (start, end) = match declaration {
            Declaration::Function(function) => {
                (function.span.start_byte, function.body.span.end_byte)
            }
            Declaration::Test(test) => (test.span.start_byte, test.body.span.end_byte),
            Declaration::Alias(alias) => (alias.span.start_byte, alias.span.end_byte),
            Declaration::Nominal(nominal) => (nominal.span.start_byte, nominal.span.end_byte),
            Declaration::Const(konst) => (konst.span.start_byte, konst.span.end_byte),
            Declaration::Resource(resource) => (resource.span.start_byte, resource.span.end_byte),
            Declaration::Struct(item) => (item.span.start_byte, item.span.end_byte),
            Declaration::Store(store) => (store.span.start_byte, store.span.end_byte),
            Declaration::Enum(item) => (item.span.start_byte, item.span.end_byte),
        };
        start as u32 <= offset && offset <= end as u32
    }

    fn locate_file<'a>(
        file: &'a SourceFile,
        offset: u32,
        scope: &mut Scope<'a>,
    ) -> Option<Located<'a>> {
        let declaration = file
            .declarations
            .iter()
            .find(|declaration| declaration_contains(declaration, offset))?;
        locate_declaration(declaration, offset, scope)
    }

    fn locate_declaration<'a>(
        declaration: &'a Declaration,
        offset: u32,
        scope: &mut Scope<'a>,
    ) -> Option<Located<'a>> {
        match declaration {
            Declaration::Function(function) => locate_function(function, offset, scope),
            Declaration::Test(test) => locate_block(&test.body, offset, scope),
            Declaration::Const(konst) => {
                if let Some(ty) = &konst.ty
                    && contains(ty.span(), offset)
                {
                    return Some(Located::TypeAnnotation);
                }
                konst
                    .value
                    .as_ref()
                    .and_then(|value| locate_expression(value, offset))
            }
            Declaration::Alias(alias) => type_position(alias.ty.as_ref(), offset),
            Declaration::Nominal(nominal) => type_position(nominal.base.as_ref(), offset),
            Declaration::Struct(item) => {
                scope.type_params = item.type_params.iter().map(|p| p.name.clone()).collect();
                members_type_position(&item.members, offset)
            }
            Declaration::Resource(resource) => members_type_position(&resource.members, offset),
            Declaration::Enum(item) => {
                scope.type_params = item.type_params.iter().map(|p| p.name.clone()).collect();
                locate_enum_payload_type(&item.members, offset)
            }
            Declaration::Store(_) => None,
        }
    }

    fn type_position(ty: Option<&TypeExpr>, offset: u32) -> Option<Located<'static>> {
        match ty {
            Some(ty) if contains(ty.span(), offset) => Some(Located::TypeAnnotation),
            _ => None,
        }
    }

    fn members_type_position(members: &[ResourceMember], offset: u32) -> Option<Located<'static>> {
        for member in members {
            match member {
                ResourceMember::Field(field) => {
                    if contains(field.ty.span(), offset) {
                        return Some(Located::TypeAnnotation);
                    }
                }
                ResourceMember::Group(group) => {
                    if let Some(located) = members_type_position(&group.members, offset) {
                        return Some(located);
                    }
                }
            }
        }
        None
    }

    fn locate_enum_payload_type(members: &[EnumMember], offset: u32) -> Option<Located<'static>> {
        for member in members {
            for field in &member.payload {
                if contains(field.ty.span(), offset) {
                    return Some(Located::TypeAnnotation);
                }
            }
            if let Some(located) = locate_enum_payload_type(&member.members, offset) {
                return Some(located);
            }
        }
        None
    }

    fn locate_function<'a>(
        function: &'a FunctionDecl,
        offset: u32,
        scope: &mut Scope<'a>,
    ) -> Option<Located<'a>> {
        scope.type_params = function
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect();
        for param in &function.params {
            if contains(param.ty.span(), offset) {
                return Some(Located::TypeAnnotation);
            }
            scope.params.push(Binding {
                name: param.name.clone(),
                ty: Some(&param.ty),
            });
        }
        if let Some(return_type) = &function.return_type
            && contains(return_type.span(), offset)
        {
            return Some(Located::TypeAnnotation);
        }
        locate_block(&function.body, offset, scope)
    }

    fn locate_block<'a>(
        block: &'a Block,
        offset: u32,
        scope: &mut Scope<'a>,
    ) -> Option<Located<'a>> {
        for statement in &block.statements {
            let span = statement.span();
            if contains(span, offset) {
                return locate_statement(statement, offset, scope);
            }
            if ends_before(span, offset)
                && let Some(binding) = following_binding(statement)
            {
                scope.locals.push(binding);
            }
        }
        None
    }

    /// The binding a statement introduces into the *following* scope (a `const`/`var`
    /// declaration and the like). Control-flow statements bind only inside their own
    /// blocks and introduce nothing here.
    fn following_binding(statement: &Statement) -> Option<Binding<'_>> {
        match statement {
            Statement::Const { name, ty, .. } | Statement::Var { name, ty, .. } => Some(Binding {
                name: name.clone(),
                ty: ty.as_deref(),
            }),
            Statement::PlaceBinding { name, .. } => Some(Binding {
                name: name.clone(),
                ty: None,
            }),
            Statement::LetElse { name, ty, .. } => Some(Binding {
                name: name.clone(),
                ty: ty.as_deref(),
            }),
            Statement::Checked { bind, .. } => match bind {
                marrow_syntax::CheckedBind::Const { name, ty, .. }
                | marrow_syntax::CheckedBind::Var { name, ty, .. } => Some(Binding {
                    name: name.clone(),
                    ty: ty.as_deref(),
                }),
                marrow_syntax::CheckedBind::Return => None,
            },
            _ => None,
        }
    }

    fn locate_statement<'a>(
        statement: &'a Statement,
        offset: u32,
        scope: &mut Scope<'a>,
    ) -> Option<Located<'a>> {
        match statement {
            Statement::Const { ty, value, .. } => {
                if let Some(ty) = ty
                    && contains(ty.span(), offset)
                {
                    return Some(Located::TypeAnnotation);
                }
                locate_expression(value, offset)
            }
            Statement::Var { ty, value, .. } => {
                if let Some(ty) = ty
                    && contains(ty.span(), offset)
                {
                    return Some(Located::TypeAnnotation);
                }
                value
                    .as_ref()
                    .and_then(|value| locate_expression(value, offset))
            }
            Statement::Assign { target, value, .. } => {
                locate_expression(target, offset).or_else(|| locate_expression(value, offset))
            }
            Statement::CompoundAssign { target, value, .. } => {
                locate_expression(target, offset).or_else(|| locate_expression(value, offset))
            }
            Statement::Delete { path, .. } => locate_expression(path, offset),
            Statement::PlaceBinding { place, .. } => locate_expression(place, offset),
            Statement::Unset { place, .. } => locate_expression(place, offset),
            Statement::Return { value, .. } => value
                .as_ref()
                .and_then(|value| locate_expression(value, offset)),
            Statement::Assert { value, .. } => locate_expression(value, offset),
            Statement::Expr { value, .. } => locate_expression(value, offset),
            Statement::Require {
                condition, value, ..
            } => locate_expression(condition, offset).or_else(|| locate_expression(value, offset)),
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(located) = locate_expression(condition, offset) {
                    return Some(located);
                }
                if contains(then_block.span, offset) {
                    return locate_block(then_block, offset, scope);
                }
                for else_if in else_ifs {
                    if let Some(located) = locate_expression(&else_if.condition, offset) {
                        return Some(located);
                    }
                    if contains(else_if.block.span, offset) {
                        return locate_block(&else_if.block, offset, scope);
                    }
                }
                else_block
                    .as_ref()
                    .filter(|block| contains(block.span, offset))
                    .and_then(|block| locate_block(block, offset, scope))
            }
            Statement::IfConst {
                name,
                ty,
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                if let Some(ty) = ty
                    && contains(ty.span(), offset)
                {
                    return Some(Located::TypeAnnotation);
                }
                if let Some(located) = locate_expression(value, offset) {
                    return Some(located);
                }
                if contains(then_block.span, offset) {
                    scope.locals.push(Binding {
                        name: name.clone(),
                        ty: ty.as_deref(),
                    });
                    return locate_block(then_block, offset, scope);
                }
                for else_if in else_ifs {
                    if let Some(located) = locate_expression(&else_if.condition, offset) {
                        return Some(located);
                    }
                    if contains(else_if.block.span, offset) {
                        return locate_block(&else_if.block, offset, scope);
                    }
                }
                else_block
                    .as_ref()
                    .filter(|block| contains(block.span, offset))
                    .and_then(|block| locate_block(block, offset, scope))
            }
            Statement::While {
                condition, body, ..
            } => locate_expression(condition, offset).or_else(|| {
                contains(body.span, offset)
                    .then(|| locate_block(body, offset, scope))
                    .flatten()
            }),
            Statement::For {
                binding,
                iterable,
                step,
                bound,
                body,
                ..
            } => {
                if let Some(located) = locate_expression(iterable, offset) {
                    return Some(located);
                }
                if let Some(step) = step
                    && let Some(located) = locate_expression(step, offset)
                {
                    return Some(located);
                }
                if let Some(bound) = bound {
                    if let Some(located) = locate_expression(&bound.limit, offset) {
                        return Some(located);
                    }
                    if let Some(from) = &bound.from
                        && let Some(located) = locate_expression(from, offset)
                    {
                        return Some(located);
                    }
                    if let Some(on_more) = &bound.on_more
                        && contains(on_more.span, offset)
                    {
                        return locate_block(on_more, offset, scope);
                    }
                }
                if contains(body.span, offset) {
                    for name in &binding.names {
                        scope.locals.push(Binding {
                            name: name.name.clone(),
                            ty: None,
                        });
                    }
                    return locate_block(body, offset, scope);
                }
                None
            }
            Statement::Transaction { body, .. } => contains(body.span, offset)
                .then(|| locate_block(body, offset, scope))
                .flatten(),
            Statement::Match {
                scrutinee, arms, ..
            } => {
                if let Some(located) = locate_expression(scrutinee, offset) {
                    return Some(located);
                }
                for arm in arms {
                    if contains(arm.block.span, offset) {
                        for arm_binding in &arm.bindings {
                            scope.locals.push(Binding {
                                name: arm_binding.name.clone(),
                                ty: None,
                            });
                        }
                        return locate_block(&arm.block, offset, scope);
                    }
                }
                None
            }
            Statement::Checked {
                bind,
                op,
                out_of_range,
                zero_divisor,
                ..
            } => {
                if let marrow_syntax::CheckedBind::Const { ty: Some(ty), .. }
                | marrow_syntax::CheckedBind::Var { ty: Some(ty), .. } = bind
                    && contains(ty.span(), offset)
                {
                    return Some(Located::TypeAnnotation);
                }
                if let Some(located) = locate_expression(op, offset) {
                    return Some(located);
                }
                for block in [out_of_range, zero_divisor].into_iter().flatten() {
                    if contains(block.span, offset) {
                        return locate_block(block, offset, scope);
                    }
                }
                None
            }
            Statement::LetElse {
                ty,
                value,
                else_block,
                ..
            } => {
                if let Some(ty) = ty
                    && contains(ty.span(), offset)
                {
                    return Some(Located::TypeAnnotation);
                }
                if let Some(located) = locate_expression(value, offset) {
                    return Some(located);
                }
                contains(else_block.span, offset)
                    .then(|| locate_block(else_block, offset, scope))
                    .flatten()
            }
            Statement::IfConstChain {
                bindings,
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                for binding in bindings {
                    if let Some(located) = locate_expression(&binding.value, offset) {
                        return Some(located);
                    }
                }
                if let Some(condition) = condition
                    && let Some(located) = locate_expression(condition, offset)
                {
                    return Some(located);
                }
                if contains(then_block.span, offset) {
                    for binding in bindings {
                        scope.locals.push(Binding {
                            name: binding.name.clone(),
                            ty: binding.ty.as_ref(),
                        });
                    }
                    return locate_block(then_block, offset, scope);
                }
                for else_if in else_ifs {
                    if contains(else_if.block.span, offset) {
                        return locate_block(&else_if.block, offset, scope);
                    }
                }
                else_block
                    .as_ref()
                    .filter(|block| contains(block.span, offset))
                    .and_then(|block| locate_block(block, offset, scope))
            }
            Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => None,
        }
    }

    /// The immediate expression children to recurse into for the compositional forms. The
    /// forms that carry a completion class of their own (`Name`, `Field`, and the recovery
    /// nodes) are matched before this helper is reached.
    fn expression_children(expression: &Expression) -> Vec<&Expression> {
        match expression {
            Expression::Call { callee, args, .. } => {
                let mut children = vec![callee.as_ref()];
                children.extend(args.iter().map(|argument| &argument.value));
                children
            }
            Expression::Keyed { base, keys, .. } => {
                let mut children = vec![base.as_ref()];
                children.extend(keys.iter());
                children
            }
            Expression::Unary { operand, .. } => vec![operand.as_ref()],
            Expression::Binary { left, right, .. } => vec![left.as_ref(), right.as_ref()],
            Expression::Membership { value, range, .. } => vec![value.as_ref(), range.as_ref()],
            Expression::Range {
                start, end, step, ..
            } => [start, end, step]
                .into_iter()
                .flatten()
                .map(|boxed| boxed.as_ref())
                .collect(),
            Expression::Interpolation { parts, .. } => parts
                .iter()
                .filter_map(|part| match part {
                    marrow_syntax::InterpolationPart::Expr(expression) => Some(expression),
                    marrow_syntax::InterpolationPart::Text { .. } => None,
                })
                .collect(),
            Expression::Try { inner, .. } => vec![inner.as_ref()],
            // Leaves carry no sub-expression; `Name`, `Field`, `OptionalField`, and
            // `Error` carry a completion class of their own and are matched before this
            // helper is reached. The match stays exhaustive so a new child-bearing
            // `Expression` variant is a compile error here rather than a silent gap.
            Expression::Literal { .. }
            | Expression::Name { .. }
            | Expression::SavedRoot { .. }
            | Expression::Absent { .. }
            | Expression::Field { .. }
            | Expression::OptionalField { .. }
            | Expression::Error { .. } => Vec::new(),
        }
    }

    fn locate_expression<'a>(expression: &'a Expression, offset: u32) -> Option<Located<'a>> {
        if !contains(expression.span(), offset) {
            return None;
        }
        match expression {
            Expression::Error {
                recovery: Some(Recovery::Member { base } | Recovery::OptionalMember { base }),
                ..
            } => {
                return if contains(base.span(), offset) {
                    locate_expression(base, offset)
                } else {
                    Some(Located::Member(base))
                };
            }
            Expression::Error {
                recovery: Some(Recovery::Path { base }),
                ..
            } => {
                return if contains(base.span(), offset) {
                    locate_expression(base, offset)
                } else {
                    Some(Located::EnumPath(base))
                };
            }
            Expression::Error { recovery: None, .. } => return None,
            Expression::Name { .. } => return Some(Located::ExprName),
            Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
                return if contains(base.span(), offset) {
                    locate_expression(base, offset)
                } else {
                    Some(Located::Member(base))
                };
            }
            _ => {}
        }
        for child in expression_children(expression) {
            if let Some(located) = locate_expression(child, offset) {
                return Some(located);
            }
        }
        None
    }

    fn expression_name_candidates(file: &SourceFile, scope: &Scope<'_>) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        for local in &scope.locals {
            candidates.push(Candidate {
                label: local.name.clone(),
                kind: CandidateKind::Local,
                detail: local.ty.map(TypeExpr::to_string).unwrap_or_default(),
            });
        }
        for param in &scope.params {
            candidates.push(Candidate {
                label: param.name.clone(),
                kind: CandidateKind::Param,
                detail: param.ty.map(TypeExpr::to_string).unwrap_or_default(),
            });
        }
        for declaration in &file.declarations {
            match declaration {
                Declaration::Function(function) => candidates.push(Candidate {
                    label: function.name.clone(),
                    kind: CandidateKind::Function,
                    detail: function_signature(function),
                }),
                Declaration::Const(konst) => candidates.push(Candidate {
                    label: konst.name.clone(),
                    kind: CandidateKind::Const,
                    detail: konst
                        .ty
                        .as_ref()
                        .map(TypeExpr::to_string)
                        .unwrap_or_default(),
                }),
                Declaration::Enum(item) => candidates.push(Candidate {
                    label: item.name.clone(),
                    kind: CandidateKind::Type,
                    detail: String::new(),
                }),
                _ => {}
            }
        }
        for name in builtin_value_names() {
            candidates.push(Candidate {
                label: (*name).to_string(),
                kind: CandidateKind::Builtin,
                detail: String::new(),
            });
        }
        for use_decl in &file.uses {
            let Some(segment) = use_decl.segments.last() else {
                continue;
            };
            candidates.push(Candidate {
                label: segment.text().to_string(),
                kind: CandidateKind::Module,
                detail: String::new(),
            });
        }
        candidates
    }

    fn member_candidates(
        file: &SourceFile,
        scope: &Scope<'_>,
        base: &Expression,
    ) -> Vec<Candidate> {
        let Some(type_name) = base_type_name(scope, base) else {
            return Vec::new();
        };
        let Some(item) = file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Struct(item) if item.name == type_name => Some(item),
                _ => None,
            })
        else {
            return Vec::new();
        };
        struct_field_candidates(&item.members)
    }

    fn struct_field_candidates(members: &[ResourceMember]) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        for member in members {
            if let ResourceMember::Field(field) = member {
                candidates.push(Candidate {
                    label: field.name.clone(),
                    kind: CandidateKind::Field,
                    detail: field.ty.to_string(),
                });
            }
        }
        candidates
    }

    /// The fail-soft type probe: the struct-type name of a single-segment base that
    /// resolves to a local or parameter annotated with a bare struct name (a
    /// single-segment [`TypeExpr::Name`]). Any partial, unannotated, generic, optional,
    /// identity, or otherwise non-bare annotation yields `None` — never a resolver
    /// failure. The name is read from the type node structurally, not from a rendered
    /// display string.
    fn base_type_name<'a>(scope: &Scope<'a>, base: &Expression) -> Option<&'a str> {
        let Expression::Name { segments, .. } = base else {
            return None;
        };
        let [name] = &segments[..] else {
            return None;
        };
        let binding = scope
            .locals
            .iter()
            .rev()
            .chain(scope.params.iter())
            .find(|binding| binding.name == name.text())?;
        match binding.ty? {
            TypeExpr::Name { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    fn enum_path_candidates(file: &SourceFile, base: &Expression) -> Vec<Candidate> {
        let Expression::Name { segments, .. } = base else {
            return Vec::new();
        };
        let Some((enum_name, rest)) = segments.split_first() else {
            return Vec::new();
        };
        let Some(item) = file
            .declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Enum(item) if item.name == enum_name.text() => Some(item),
                _ => None,
            })
        else {
            return Vec::new();
        };
        match resolve_enum_members(item, rest) {
            Some(members) => members
                .iter()
                .map(|member| Candidate {
                    label: member.name.clone(),
                    kind: CandidateKind::EnumMember {
                        selectable: !member.category,
                    },
                    detail: String::new(),
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// Walk the qualified segments after the enum name into the member tree, returning the
    /// reached node's immediate members. An unresolvable segment yields `None`.
    fn resolve_enum_members<'a>(
        item: &'a EnumDecl,
        rest: &[NameSegment],
    ) -> Option<&'a [EnumMember]> {
        let mut members = item.members.as_slice();
        for segment in rest {
            let member = members
                .iter()
                .find(|member| member.name == segment.text())?;
            members = member.members.as_slice();
        }
        Some(members)
    }

    fn type_annotation_candidates(file: &SourceFile, scope: &Scope<'_>) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        for declaration in &file.declarations {
            let name = match declaration {
                Declaration::Alias(item) => &item.name,
                Declaration::Nominal(item) => &item.name,
                Declaration::Struct(item) => &item.name,
                Declaration::Enum(item) => &item.name,
                Declaration::Resource(item) => &item.name,
                _ => continue,
            };
            candidates.push(Candidate {
                label: name.clone(),
                kind: CandidateKind::Type,
                detail: String::new(),
            });
        }
        for name in builtin_type_names() {
            candidates.push(Candidate {
                label: name.to_string(),
                kind: CandidateKind::Type,
                detail: String::new(),
            });
        }
        for type_param in &scope.type_params {
            candidates.push(Candidate {
                label: type_param.clone(),
                kind: CandidateKind::TypeParam,
                detail: String::new(),
            });
        }
        candidates
    }

    /// The built-in type-name namespace: the language scalar spellings (routed through the
    /// scalar owner), the reserved toolchain generics (routed through their type-system
    /// owner so the completion set cannot drift from the redeclaration gate), and the `Id`
    /// identity-type keyword.
    fn builtin_type_names() -> Vec<&'static str> {
        let mut names: Vec<&'static str> = [
            ScalarType::Int,
            ScalarType::Bool,
            ScalarType::Text,
            ScalarType::Bytes,
            ScalarType::Date,
            ScalarType::Instant,
            ScalarType::Duration,
        ]
        .into_iter()
        .map(ScalarType::spelling)
        .collect();
        names.extend(crate::types::RESERVED_GENERIC_TYPE_NAMES);
        names.push("Id");
        names
    }

    fn function_signature(function: &FunctionDecl) -> String {
        let mut signature = String::from("(");
        for (index, param) in function.params.iter().enumerate() {
            if index > 0 {
                signature.push_str(", ");
            }
            signature.push_str(&param.name);
            signature.push_str(": ");
            signature.push_str(&param.ty.to_string());
        }
        signature.push(')');
        if let Some(return_type) = &function.return_type {
            signature.push_str(": ");
            signature.push_str(&return_type.to_string());
        }
        signature
    }
}

/// The per-query read-only active-call resolution.
///
/// Like [`completion`], a distinct read-only pass over the query-local parse: it never
/// drives the compile-path resolver, so it runs on a broken file over recovered
/// incomplete-call nodes and leaks no diagnostic. It collects the calls in the enclosing
/// declaration, selects the innermost whose argument region holds the offset, resolves the
/// callee to a same-module function or generic template in that parse, renders the
/// callee's canonical signature and parameter pieces from the declared spellings, and
/// computes the active argument index positionally. A cross-module callee, a built-in, or
/// an unknown name resolves to no local declaration and is a legitimate absence.
mod active_call {
    use marrow_syntax::{
        Argument, Block, Declaration, Expression, FunctionDecl, InterpolationPart, SourceFile,
        SourceSpan, Statement,
    };

    use super::completion::{contains, declaration_contains};
    use super::{
        ActiveCall, ActiveCallOutcome, AnalysisResourceLimit, Fact, MAX_ACTIVE_CALL_RENDER_BYTES,
        ParamPiece,
    };

    /// One call node reached during collection: its callee expression, its arguments
    /// parsed so far, and its full span. A recovered incomplete call carries the arguments
    /// parsed before the missing delimiter and a span ending at the last parsed token.
    struct CallSite<'a> {
        callee: &'a Expression,
        args: &'a [Argument],
        span: SourceSpan,
    }

    pub(super) fn resolve(file: &SourceFile, source: &[u8], offset: u32) -> ActiveCallOutcome {
        let Some(declaration) = file
            .declarations
            .iter()
            .find(|declaration| declaration_contains(declaration, offset))
        else {
            return ActiveCallOutcome::Ready(Fact::Absent);
        };
        let mut sites = Vec::new();
        collect_declaration_calls(declaration, &mut sites);
        // The innermost enclosing call is the smallest-span call whose argument region
        // holds the offset. A recovered incomplete call extends its region across trailing
        // whitespace to the cursor, so the just-opened `f(` and just-typed `f(a, ` moments
        // still resolve.
        let Some(site) = sites
            .into_iter()
            .filter(|site| region_contains(site, source, offset))
            .min_by_key(|site| site.span.end_byte - site.span.start_byte)
        else {
            return ActiveCallOutcome::Ready(Fact::Absent);
        };
        let Some(function) = resolve_callee(file, site.callee) else {
            return ActiveCallOutcome::Ready(Fact::Absent);
        };
        let (signature, params) = render_signature(function);
        let active = active_index(&site, offset, params.len());
        finish(signature, params, active)
    }

    /// Apply the per-query rendered-byte cap, then package the fact. An over-cap display is
    /// a query-local refusal, never a truncated display.
    fn finish(
        signature: String,
        params: Vec<ParamPiece>,
        active: Option<u16>,
    ) -> ActiveCallOutcome {
        let bytes = signature.len() as u64
            + params
                .iter()
                .map(|piece| piece.label.len() as u64)
                .sum::<u64>();
        if bytes > MAX_ACTIVE_CALL_RENDER_BYTES {
            return ActiveCallOutcome::Refused(AnalysisResourceLimit::ActiveCallRenderBytes {
                limit: MAX_ACTIVE_CALL_RENDER_BYTES,
            });
        }
        ActiveCallOutcome::Ready(Fact::Present(ActiveCall {
            signature,
            params,
            active,
        }))
    }

    /// Whether a call's argument region holds the offset. The region opens just past the
    /// callee (the `(` and beyond) and closes at the parsed extent; a recovered,
    /// unterminated call (whose last byte is not `)`) additionally reaches the cursor
    /// across trailing whitespace only.
    fn region_contains(site: &CallSite, source: &[u8], offset: u32) -> bool {
        let callee_end = site.callee.span().end_byte as u32;
        if offset <= callee_end {
            return false;
        }
        if offset <= site.span.end_byte as u32 {
            return true;
        }
        let end = site.span.end_byte;
        // A terminated call ends in its `)`; the cursor is then past the closed call.
        if end > 0 && source.get(end - 1) == Some(&b')') {
            return false;
        }
        match source.get(end..offset as usize) {
            Some(gap) => gap.iter().all(u8::is_ascii_whitespace),
            None => false,
        }
    }

    /// The active argument index the offset sits at, or `None` when the callee declares no
    /// parameters. A cursor inside an argument's own extent is that argument's slot;
    /// otherwise the slot is the count of arguments whose extent ends before the offset —
    /// the position the next argument would occupy.
    fn active_index(site: &CallSite, offset: u32, param_count: usize) -> Option<u16> {
        if param_count == 0 {
            return None;
        }
        for (index, argument) in site.args.iter().enumerate() {
            if contains(argument.value.span(), offset) {
                return Some(index as u16);
            }
        }
        let index = site
            .args
            .iter()
            .filter(|argument| (argument.value.span().end_byte as u32) < offset)
            .count();
        Some(index as u16)
    }

    /// Resolve a callee expression to a same-module function or generic template in this
    /// file's parse. A qualified (cross-module) name, a built-in, or an unknown name
    /// resolves to no local declaration on this floor.
    fn resolve_callee<'a>(file: &'a SourceFile, callee: &Expression) -> Option<&'a FunctionDecl> {
        let Expression::Name { segments, .. } = callee else {
            return None;
        };
        let [name] = &segments[..] else {
            return None;
        };
        file.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if function.name == name.text() => Some(function),
                _ => None,
            })
    }

    /// The callee's canonical signature display and its parameter pieces, rendered from the
    /// declared spellings. A generic callee carries its template `<...>` parameters. Each
    /// piece composes the signature (`fn name<T>(piece, piece): ret`).
    fn render_signature(function: &FunctionDecl) -> (String, Vec<ParamPiece>) {
        let params: Vec<ParamPiece> = function
            .params
            .iter()
            .map(|param| ParamPiece {
                label: format!("{}: {}", param.name, param.ty),
            })
            .collect();
        let type_params = if function.type_params.is_empty() {
            String::new()
        } else {
            let names = function
                .type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("<{names}>")
        };
        let joined = params
            .iter()
            .map(|piece| piece.label.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let signature = match &function.return_type {
            None => format!("fn {}{type_params}({joined})", function.name),
            Some(return_type) => {
                format!("fn {}{type_params}({joined}): {return_type}", function.name)
            }
        };
        (signature, params)
    }

    fn collect_declaration_calls<'a>(declaration: &'a Declaration, sink: &mut Vec<CallSite<'a>>) {
        match declaration {
            Declaration::Function(function) => collect_block_calls(&function.body, sink),
            Declaration::Test(test) => collect_block_calls(&test.body, sink),
            Declaration::Const(konst) => {
                if let Some(value) = &konst.value {
                    collect_expression_calls(value, sink);
                }
            }
            // No other declaration carries call expressions in value position on this
            // floor: types, resources, stores, and enums are structural.
            Declaration::Alias(_)
            | Declaration::Nominal(_)
            | Declaration::Resource(_)
            | Declaration::Struct(_)
            | Declaration::Store(_)
            | Declaration::Enum(_) => {}
        }
    }

    fn collect_block_calls<'a>(block: &'a Block, sink: &mut Vec<CallSite<'a>>) {
        for statement in &block.statements {
            collect_statement_calls(statement, sink);
        }
    }

    fn collect_statement_calls<'a>(statement: &'a Statement, sink: &mut Vec<CallSite<'a>>) {
        match statement {
            Statement::Const { value, .. } => collect_expression_calls(value, sink),
            Statement::Var { value, .. } => {
                if let Some(value) = value {
                    collect_expression_calls(value, sink);
                }
            }
            Statement::Assign { target, value, .. }
            | Statement::CompoundAssign { target, value, .. } => {
                collect_expression_calls(target, sink);
                collect_expression_calls(value, sink);
            }
            Statement::Delete { path, .. } => collect_expression_calls(path, sink),
            Statement::PlaceBinding { place, .. } | Statement::Unset { place, .. } => {
                collect_expression_calls(place, sink)
            }
            Statement::Return { value, .. } => {
                if let Some(value) = value {
                    collect_expression_calls(value, sink);
                }
            }
            Statement::Assert { value, .. } | Statement::Expr { value, .. } => {
                collect_expression_calls(value, sink)
            }
            Statement::Require {
                condition, value, ..
            } => {
                collect_expression_calls(condition, sink);
                collect_expression_calls(value, sink);
            }
            Statement::If {
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_expression_calls(condition, sink);
                collect_block_calls(then_block, sink);
                for else_if in else_ifs {
                    collect_expression_calls(&else_if.condition, sink);
                    collect_block_calls(&else_if.block, sink);
                }
                if let Some(else_block) = else_block {
                    collect_block_calls(else_block, sink);
                }
            }
            Statement::IfConst {
                value,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                collect_expression_calls(value, sink);
                collect_block_calls(then_block, sink);
                for else_if in else_ifs {
                    collect_expression_calls(&else_if.condition, sink);
                    collect_block_calls(&else_if.block, sink);
                }
                if let Some(else_block) = else_block {
                    collect_block_calls(else_block, sink);
                }
            }
            Statement::IfConstChain {
                bindings,
                condition,
                then_block,
                else_ifs,
                else_block,
                ..
            } => {
                for binding in bindings {
                    collect_expression_calls(&binding.value, sink);
                }
                if let Some(condition) = condition {
                    collect_expression_calls(condition, sink);
                }
                collect_block_calls(then_block, sink);
                for else_if in else_ifs {
                    collect_expression_calls(&else_if.condition, sink);
                    collect_block_calls(&else_if.block, sink);
                }
                if let Some(else_block) = else_block {
                    collect_block_calls(else_block, sink);
                }
            }
            Statement::LetElse {
                value, else_block, ..
            } => {
                collect_expression_calls(value, sink);
                collect_block_calls(else_block, sink);
            }
            Statement::While {
                condition, body, ..
            } => {
                collect_expression_calls(condition, sink);
                collect_block_calls(body, sink);
            }
            Statement::For {
                iterable,
                step,
                bound,
                body,
                ..
            } => {
                collect_expression_calls(iterable, sink);
                if let Some(step) = step {
                    collect_expression_calls(step, sink);
                }
                if let Some(bound) = bound {
                    collect_expression_calls(&bound.limit, sink);
                    if let Some(from) = &bound.from {
                        collect_expression_calls(from, sink);
                    }
                    if let Some(on_more) = &bound.on_more {
                        collect_block_calls(on_more, sink);
                    }
                }
                collect_block_calls(body, sink);
            }
            Statement::Transaction { body, .. } => collect_block_calls(body, sink),
            Statement::Match {
                scrutinee, arms, ..
            } => {
                collect_expression_calls(scrutinee, sink);
                for arm in arms {
                    collect_block_calls(&arm.block, sink);
                }
            }
            Statement::Checked {
                op,
                out_of_range,
                zero_divisor,
                ..
            } => {
                collect_expression_calls(op, sink);
                for block in [out_of_range, zero_divisor].into_iter().flatten() {
                    collect_block_calls(block, sink);
                }
            }
            Statement::Break { .. } | Statement::Continue { .. } | Statement::Error { .. } => {}
        }
    }

    /// Collect every call in an expression and its sub-expressions. Unlike the completion
    /// module's `expression_children`, this descends into a field receiver and every other
    /// compositional form so a call nested anywhere under the expression is reached.
    fn collect_expression_calls<'a>(expression: &'a Expression, sink: &mut Vec<CallSite<'a>>) {
        match expression {
            Expression::Call {
                callee, args, span, ..
            } => {
                sink.push(CallSite {
                    callee: callee.as_ref(),
                    args,
                    span: *span,
                });
                collect_expression_calls(callee, sink);
                for argument in args {
                    collect_expression_calls(&argument.value, sink);
                }
            }
            Expression::Keyed { base, keys, .. } => {
                collect_expression_calls(base, sink);
                for key in keys {
                    collect_expression_calls(key, sink);
                }
            }
            Expression::Field { base, .. } | Expression::OptionalField { base, .. } => {
                collect_expression_calls(base, sink);
            }
            Expression::Unary { operand, .. } => collect_expression_calls(operand, sink),
            Expression::Binary { left, right, .. } => {
                collect_expression_calls(left, sink);
                collect_expression_calls(right, sink);
            }
            Expression::Membership { value, range, .. } => {
                collect_expression_calls(value, sink);
                collect_expression_calls(range, sink);
            }
            Expression::Range {
                start, end, step, ..
            } => {
                for part in [start, end, step].into_iter().flatten() {
                    collect_expression_calls(part, sink);
                }
            }
            Expression::Interpolation { parts, .. } => {
                for part in parts {
                    if let InterpolationPart::Expr(expression) = part {
                        collect_expression_calls(expression, sink);
                    }
                }
            }
            Expression::Try { inner, .. } => collect_expression_calls(inner, sink),
            // Leaves carry no sub-expression. The match stays exhaustive so a new
            // child-bearing `Expression` variant is a compile error here rather than a
            // silently unreached nested call.
            Expression::Literal { .. }
            | Expression::Name { .. }
            | Expression::SavedRoot { .. }
            | Expression::Absent { .. }
            | Expression::Error { .. } => {}
        }
    }
}

#[cfg(test)]
mod fact_ledger_tests {
    use super::*;
    use crate::diag::{MAX_DIAGNOSTIC_BYTES, MAX_DIAGNOSTIC_COUNT};
    use marrow_project::{CaptureLimits, CapturedFile, Manifest};
    use std::mem::size_of;

    /// The accounted physical footprint of one live [`AnalysisSnapshot`], **excluding**
    /// the caller-shared `Arc<ProjectInput>`: its up-to-64 MiB of source bytes are the
    /// caller's charge, shared not copied, and are named separately for the editor
    /// session that holds it.
    ///
    /// The exported term this must not exceed. It is an arithmetic property of the
    /// pinned ceilings and the retained representation, not a runtime check.
    const MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES: u64 = 12 * 1024 * 1024;

    /// The admitted per-file byte ceiling is inside the retained span coordinate
    /// domain, so every span a snapshot retains round-trips exactly. A widened
    /// admission ceiling must widen [`FactSpan`] with it; this pins the two together
    /// exactly as the diagnostic owner pins its ceilings against the syntax owner's.
    #[test]
    fn the_admission_ceiling_fits_the_fact_coordinate_domain() {
        assert!(CaptureLimits::DEFAULT.max_file_bytes() as u64 <= u32::MAX as u64);
        let widest = SourceSpan {
            start_byte: CaptureLimits::DEFAULT.max_file_bytes() - 1,
            end_byte: CaptureLimits::DEFAULT.max_file_bytes(),
            line: u32::MAX,
            column: u32::MAX,
        };
        assert_eq!(FactSpan::of(widest).source(), widest);
    }

    /// The per-file admission ceiling every coordinate and span is inside.
    fn max_files() -> u64 {
        CaptureLimits::DEFAULT.max_files() as u64
    }

    /// The largest footprint any admissible snapshot can reach, derived field by field
    /// from the retained representation.
    ///
    /// The count-bounded families — hover facts, dependency gaps, and document-symbol
    /// nodes — share one ceiling, so charging the whole ceiling at the widest of their
    /// unit sizes bounds every mixture of them.
    fn worst_case_retained_bytes(fact_unit: u64, symbol_outline_unit: u64) -> u64 {
        MAX_SNAPSHOT_FACT_COUNT * fact_unit
            + MAX_SNAPSHOT_FACT_BYTES
            + MAX_DIAGNOSTIC_BYTES as u64
            + MAX_DIAGNOSTIC_COUNT as u64 * size_of::<SourceDiagnostic>() as u64
            + max_files() * symbol_outline_unit
            + max_files() * size_of::<FileRef>() as u64
    }

    /// The widest retained unit charged against the shared fact count. A hover fact
    /// carries its definition target inline, so one admitted count charges one struct
    /// and never a second retained row.
    fn fact_unit() -> u64 {
        [
            size_of::<HoverFact>(),
            size_of::<DeclSymbol>(),
            size_of::<(FileRef, FactSpan)>(),
        ]
        .into_iter()
        .max()
        .unwrap_or(0) as u64
    }

    /// The exact accounted worst case, published as a figure in the implementation map.
    /// A change to it is an observable-contract change, so it is asserted rather than
    /// only bounded.
    const ACCOUNTED_WORST_CASE_RETAINED_BYTES: u64 = 11_116_544;

    /// The accounted footprint closes under the exported term with the compact
    /// representation.
    ///
    /// Two drift gates keep the accounting honest between them. Here, the snapshot
    /// destructure is exhaustive, so a new retained *field* is a build error rather than
    /// a silent term violation. At each retained fact type, `retained_bytes` destructures
    /// its own fields exhaustively, so a new heap-owning field on `HoverFact`,
    /// `DefinitionTarget`, or `DeclSymbol` is a build error there rather than retention
    /// no ceiling and no term ever sees.
    #[test]
    fn the_accounted_footprint_closes_under_the_exported_term() {
        let AnalysisSnapshot {
            input: _,
            revision: _,
            diagnostics: _,
            hover_facts: _,
            broken_files: _,
            dependency_gaps: _,
            document_symbols: _,
            symbol_bounded_files: _,
        } = empty_snapshot();

        let accounted = worst_case_retained_bytes(
            fact_unit(),
            size_of::<(FileRef, Box<[DeclSymbol]>)>() as u64,
        );
        assert_eq!(
            accounted, ACCOUNTED_WORST_CASE_RETAINED_BYTES,
            "the exact accounted worst case is published; update the implementation map \
             and this pin together"
        );
        assert!(
            accounted <= MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES,
            "accounted {accounted} exceeds the exported \
             MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES {MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES}"
        );
    }

    /// The exported term for the peak attributable to producing facts.
    const MAX_ANALYSIS_FACT_TRANSIENT_BYTES: u64 = 25 * 1024 * 1024;

    /// Amortized growth plus the one buffer a `Vec` still holds while it copies into its
    /// successor: a growing collection is live at three times its admitted length.
    const GROWTH_AND_COPY: u64 = 3;

    /// The **live fact payload** — everything the ledger holds while it is retaining — is
    /// an arithmetic property of its own ceilings, not a property of the workload that
    /// filled it.
    ///
    /// This is what keeps `MAX_ANALYSIS_FACT_TRANSIENT_BYTES` from being hostage to
    /// fixture choice. Admission stops at [`MAX_SNAPSHOT_FACT_COUNT`] and
    /// [`MAX_SNAPSHOT_FACT_BYTES`] at the push that produced each fact, so no body,
    /// however wide, can make the payload larger than this; a denser fixture can only
    /// reach the ceiling sooner. Every charged spelling is held in an exactly sized
    /// `Box<str>`, so the byte ceiling is the physical figure and not only a logical one.
    ///
    /// What it does **not** cover, and what the lane's measured differential does: the
    /// producer-side rendering of one display at a time, which is built and freed as it
    /// is charged, and the checker's own working set, which is not fact state.
    #[test]
    fn the_live_fact_payload_is_bounded_by_the_ledger_ceilings() {
        // Every counted family shares one ceiling, so charging the whole ceiling at the
        // widest of their unit sizes bounds every mixture of them.
        let counted = GROWTH_AND_COPY * MAX_SNAPSHOT_FACT_COUNT * fact_unit();
        // Broken-module status and the per-module spelling table are bounded by project
        // admission, not by the fact count.
        let per_file = GROWTH_AND_COPY
            * max_files()
            * (size_of::<FileRef>() as u64
                + size_of::<u32>() as u64
                + size_of::<(FileRef, Box<[DeclSymbol]>)>() as u64);
        // One module's outline is live before it is charged, bounded by its own per-file
        // node ceiling and dropped as it is admitted.
        let outline =
            GROWTH_AND_COPY * MAX_DOCUMENT_SYMBOLS_PER_FILE * size_of::<DeclSymbol>() as u64;
        let payload = counted + MAX_SNAPSHOT_FACT_BYTES + per_file + outline;
        assert_eq!(
            payload, 21_176_320,
            "the accounted live fact payload moved; re-derive the exported transient \
             term before changing this number"
        );
        assert!(
            payload <= MAX_ANALYSIS_FACT_TRANSIENT_BYTES,
            "accounted live fact payload {payload} exceeds the exported \
             MAX_ANALYSIS_FACT_TRANSIENT_BYTES {MAX_ANALYSIS_FACT_TRANSIENT_BYTES}"
        );
    }

    /// The compaction is load-bearing, not cosmetic: the same accounting against the
    /// representation this row replaced does **not** close.
    ///
    /// That representation retained, per hover fact, an owned `FileIdentity` (24 bytes
    /// plus up to 4096 bytes of spelling, charged by neither ceiling) and a definition
    /// target holding a second one — a 144-byte fact struct against today's 80 — and
    /// grew every collection as a `Vec` whose amortized capacity was retained with it.
    #[test]
    fn the_superseded_representation_does_not_close() {
        const SUPERSEDED_HOVER_FACT: u64 = 144;
        const SUPERSEDED_DECL_SYMBOL: u64 = 104;
        const SUPERSEDED_OUTLINE_ENTRY: u64 = 48;
        const SUPERSEDED_DEFINITION_TARGET: u64 = 72;

        let unit = SUPERSEDED_HOVER_FACT.max(SUPERSEDED_DECL_SYMBOL);
        let superseded = MAX_SNAPSHOT_FACT_COUNT * unit
            + MAX_SNAPSHOT_FACT_BYTES
            + MAX_DIAGNOSTIC_BYTES as u64
            + MAX_DIAGNOSTIC_COUNT as u64 * size_of::<SourceDiagnostic>() as u64
            + max_files() * SUPERSEDED_OUTLINE_ENTRY
            + MAX_SNAPSHOT_FACT_COUNT * SUPERSEDED_DEFINITION_TARGET;
        assert!(
            superseded > MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES,
            "the superseded representation must not close, or the compaction is not \
             load-bearing: {superseded}"
        );

        // The uncharged per-fact identity spelling alone dwarfs the whole term.
        let uncharged = MAX_SNAPSHOT_FACT_COUNT * marrow_project::MAX_FILE_IDENTITY_BYTES as u64;
        assert!(uncharged > 16 * MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES);
    }

    fn project(files: &[(&str, &str)]) -> ProjectInput {
        let manifest = Manifest::parse("edition = \"2026\"\n").expect("valid manifest");
        let captured = files
            .iter()
            .map(|(path, source)| {
                CapturedFile::new((*path).to_string(), source.as_bytes().to_vec())
            })
            .collect();
        marrow_project::capture(&manifest, captured, None, &CaptureLimits::DEFAULT)
            .expect("capture the fixture project")
    }

    fn empty_snapshot() -> AnalysisSnapshot {
        AnalysisSnapshot {
            input: Arc::new(project(&[("src/main.mw", "")])),
            revision: InputRevision::new(0),
            diagnostics: Box::default(),
            hover_facts: Box::default(),
            broken_files: Box::default(),
            dependency_gaps: Box::default(),
            document_symbols: Box::default(),
            symbol_bounded_files: Box::default(),
        }
    }

    fn ledger(input: &ProjectInput) -> AnalysisFactCollector {
        AnalysisFactCollector::new(input)
    }

    fn first() -> FileRef {
        FileRef::at(0).expect("index zero is a coordinate")
    }

    fn span(start: usize, end: usize) -> SourceSpan {
        SourceSpan {
            start_byte: start,
            end_byte: end,
            line: 1,
            column: 1,
        }
    }

    fn leaf(name: &str) -> DeclSymbol {
        DeclSymbol {
            name: name.into(),
            kind: DeclKind::Function,
            name_span: FactSpan::of(span(0, 1)),
            full_range: FactSpan::of(span(0, 2)),
            children: Box::default(),
        }
    }

    /// A hover fact charges its display plus the file spelling of its optional
    /// definition target — the logical charge, unchanged by the compact representation
    /// that no longer stores that spelling per fact.
    #[test]
    fn a_hover_fact_charges_its_display_and_its_target_spelling() {
        let input = project(&[("src/main.mw", "")]);
        let spelling = input.modules()[0].identity().as_str().len() as u64;

        let mut plain = ledger(&input);
        plain.admit_hover(first(), span(0, 1), "int".into(), None);
        assert_eq!(charged_bytes(&plain), 3);

        let mut targeted = ledger(&input);
        targeted.admit_hover(
            first(),
            span(0, 1),
            "int".into(),
            Some(DefinitionTarget::new(first(), span(4, 8), span(0, 20))),
        );
        assert_eq!(charged_bytes(&targeted), 3 + spelling);
    }

    /// A dependency gap carries only fixed-size references and a span, so the count
    /// bound charges it and it charges no bytes.
    #[test]
    fn a_dependency_gap_charges_only_the_count() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        facts.admit_gap(first(), span(0, 4));
        assert_eq!(charged(&facts), (1, 0));
    }

    /// A document-symbol module charges one count per projected node, counting nested
    /// members, and its owner file spelling once plus every retained name spelling.
    #[test]
    fn a_symbol_outline_charges_its_owner_spelling_once_and_every_name() {
        let input = project(&[("src/main.mw", "")]);
        let spelling = input.modules()[0].identity().as_str().len() as u64;
        let mut facts = ledger(&input);
        let nested = DeclSymbol {
            name: "outer".into(),
            kind: DeclKind::Enum,
            name_span: FactSpan::of(span(0, 5)),
            full_range: FactSpan::of(span(0, 20)),
            children: Box::new([leaf("inner")]),
        };
        facts.admit_symbols(first(), Box::new([nested, leaf("solo")]));
        assert_eq!(
            charged(&facts),
            (
                3,
                spelling + "outer".len() as u64 + "inner".len() as u64 + "solo".len() as u64
            )
        );
    }

    /// One admitted count charges exactly one retained fact, whether or not the fact
    /// names a definition target: the target is carried inside the fact, so no second
    /// retained row and no second maximum exists for the exported term to charge.
    #[test]
    fn a_hover_fact_carries_its_definition_target() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        let target = DefinitionTarget::new(first(), span(4, 8), span(0, 20));
        for start in 0..16 {
            facts.admit_hover(first(), span(start, start + 1), "int".into(), Some(target));
        }
        let (count, bytes) = charged(&facts);
        assert_eq!(count, 16, "one count per admitted fact, never two");
        // "int" plus the one file spelling each target charges.
        let spelling = facts.spelling_bytes(first());
        assert_eq!(bytes, 16 * (3 + spelling));
        let retained = match facts.finish() {
            BoundedAnalysisFacts::Complete(facts) => facts,
            BoundedAnalysisFacts::Limited { .. } => {
                panic!("the fixture is far under both ceilings")
            }
        };
        assert_eq!(retained.hover_facts.len(), 16);
        assert!(
            retained
                .hover_facts
                .iter()
                .all(|fact| fact.definition == Some(target))
        );
    }

    /// Crossing the count ceiling discards the whole payload, including the admitted
    /// prefix, and never re-materializes it.
    #[test]
    fn crossing_the_count_discards_the_admitted_prefix() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        for index in 0..MAX_SNAPSHOT_FACT_COUNT {
            facts.admit_gap(first(), span(index as usize, index as usize + 1));
        }
        assert!(!facts.is_limited(), "the ceiling itself is admitted");
        facts.admit_gap(first(), span(0, 1));
        assert!(facts.is_limited());
        // A later admission composes into the limited state; nothing re-materializes.
        facts.admit_hover(first(), span(0, 1), "int".into(), None);
        assert!(matches!(
            facts.finish(),
            BoundedAnalysisFacts::Limited {
                limit: AnalysisFactLimit::Count { limit }
            } if limit == MAX_SNAPSHOT_FACT_COUNT
        ));
    }

    /// Crossing the byte ceiling reports the byte limit, and the ledger retains
    /// nothing.
    #[test]
    fn crossing_the_bytes_reports_the_byte_limit() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        let chunk = MAX_SNAPSHOT_FACT_BYTES as usize / 8;
        for _ in 0..9 {
            facts.admit_hover(first(), span(0, 1), "x".repeat(chunk).into(), None);
        }
        assert!(matches!(
            facts.finish(),
            BoundedAnalysisFacts::Limited {
                limit: AnalysisFactLimit::Bytes { limit }
            } if limit == MAX_SNAPSHOT_FACT_BYTES
        ));
    }

    /// Count wins a simultaneous crossing.
    #[test]
    fn count_wins_a_simultaneous_crossing() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        let display = "x".repeat(MAX_SNAPSHOT_FACT_BYTES as usize + 1);
        for _ in 0..MAX_SNAPSHOT_FACT_COUNT {
            facts.admit_gap(first(), span(0, 1));
        }
        facts.admit_hover(first(), span(0, 1), display.into(), None);
        assert!(matches!(
            facts.finish(),
            BoundedAnalysisFacts::Limited {
                limit: AnalysisFactLimit::Count { .. }
            }
        ));
    }

    /// A Bytes limit strengthens to Count once the composed count crosses; Count never
    /// weakens back to Bytes.
    #[test]
    fn bytes_strengthens_to_count_and_count_never_weakens() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        facts.admit_hover(
            first(),
            span(0, 1),
            "x".repeat(MAX_SNAPSHOT_FACT_BYTES as usize + 1).into(),
            None,
        );
        assert!(facts.is_limited());
        for index in 0..=MAX_SNAPSHOT_FACT_COUNT {
            facts.admit_gap(first(), span(index as usize, index as usize + 1));
        }
        assert!(matches!(
            facts.finish(),
            BoundedAnalysisFacts::Limited {
                limit: AnalysisFactLimit::Count { .. }
            }
        ));

        let mut reverse = ledger(&input);
        for index in 0..=MAX_SNAPSHOT_FACT_COUNT {
            reverse.admit_gap(first(), span(index as usize, index as usize + 1));
        }
        reverse.admit_hover(
            first(),
            span(0, 1),
            "x".repeat(MAX_SNAPSHOT_FACT_BYTES as usize + 1).into(),
            None,
        );
        assert!(matches!(
            reverse.finish(),
            BoundedAnalysisFacts::Limited {
                limit: AnalysisFactLimit::Count { .. }
            }
        ));
    }

    /// The count ceiling bounds one body, not only one project. Every hover fact is
    /// admitted at the push that produced it, so a ceiling reached part-way through a
    /// body stops that body's remaining display renders immediately. A producer that
    /// staged its body's facts in a vector of its own would render — and hold — every one
    /// of them before the ledger saw the first, making one body's live fact set a
    /// function of the body's length rather than of the ceiling.
    ///
    /// The differential is the proof: the same wide body renders thousands of characters
    /// of display when the ledger has room, and almost none when the ledger is already at
    /// the ceiling as the body begins.
    #[test]
    fn one_body_stops_rendering_hover_displays_at_the_ceiling() {
        // Document-symbol nodes are admitted before any body lowers, so they fill the
        // shared count to one short of the ceiling.
        let prefill = MAX_SNAPSHOT_FACT_COUNT as usize - 1;
        let per_file = 4_001usize;
        let mut sources: Vec<(String, String)> = Vec::new();
        let mut filled = 0usize;
        while filled < prefill {
            let members = per_file.min(prefill - filled) - 1;
            let index = sources.len();
            let mut source = format!("module module_{index}\n\nenum E {{\n");
            for member in 0..members {
                source.push_str(&format!("    m{member}\n"));
            }
            source.push_str("}\n");
            sources.push((format!("src/module_{index}.mw"), source));
            filled += members + 1;
        }
        assert_eq!(filled, prefill);

        let mut wide = String::from("module wide\n\nfn wide(a: int): int {\n    var t: int = a\n");
        for _ in 0..2_000 {
            wide.push_str("    t = t + t\n");
        }
        wide.push_str("    return t\n}\n");

        let rendered = |files: Vec<(&str, &str)>| {
            let input = Arc::new(project(&files));
            let (_, counts) = crate::types::capture_scaling_counts(|| {
                let _ = crate::analysis::analyze(input, InputRevision::new(1));
            });
            counts.hover_spelling_chars
        };

        let mut files: Vec<(&str, &str)> = sources
            .iter()
            .map(|(path, source)| (path.as_str(), source.as_str()))
            .collect();
        files.push(("src/wide.mw", wide.as_str()));
        let at_the_ceiling = rendered(files);
        let with_room = rendered(vec![("src/wide.mw", wide.as_str())]);

        assert!(
            with_room > 4_000,
            "the fixture body renders thousands of display characters when the ledger \
             has room: {with_room}"
        );
        assert!(
            at_the_ceiling < 64,
            "a body whose facts cross the ceiling must stop rendering displays inside \
             itself, not after it: {at_the_ceiling} characters rendered"
        );
    }

    /// Broken-module status is not a public fact row: it charges neither ceiling and is
    /// bounded by the same file admission limit that bounds the coordinate domain.
    #[test]
    fn broken_status_charges_neither_ceiling() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        facts.admit_broken(first());
        assert_eq!(charged(&facts), (0, 0));
    }

    /// Every retained fact's span indexes only the snapshot's own bytes for the file it
    /// names, and every retained coordinate resolves to a module of that same input.
    /// A coordinate is validated against those bytes and nothing else; agreement with an
    /// editor's live document text is a separate owner's obligation and is not claimed.
    #[test]
    fn every_retained_span_lies_inside_its_own_file() {
        let source = "module main\n\n\
             fn helper(v: int): int {\n    return v\n}\n\n\
             pub fn run(): int {\n    var a: int = 2\n    return helper(a)\n}\n";
        let other = "module other\n\npub fn ping(): int {\n    return 1\n}\n";
        let input = Arc::new(project(&[("src/main.mw", source), ("src/other.mw", other)]));
        let snapshot = crate::analysis::analyze(Arc::clone(&input), InputRevision::new(3))
            .unwrap_or_else(|_| panic!("the fixture analyzes"));

        let extent = |at: FileRef| {
            snapshot
                .identity_of(at)
                .and_then(|identity| snapshot.locate(identity).ok())
                .map(|(_, bytes)| bytes.len())
                .unwrap_or_else(|| panic!("a retained coordinate names an input module"))
        };
        assert!(
            !snapshot.hover_facts.is_empty(),
            "the fixture retains facts"
        );
        for fact in &snapshot.hover_facts {
            let len = extent(fact.file);
            assert!(fact.span.start <= fact.span.end);
            assert!(
                fact.span.end as usize <= len,
                "a fact span leaves its own file"
            );
        }
        for (file, span) in &snapshot.dependency_gaps {
            assert!(span.end as usize <= extent(*file));
        }
        for target in snapshot
            .hover_facts
            .iter()
            .filter_map(|fact| fact.definition)
        {
            let len = extent(target.file);
            assert!(target.name_span.end as usize <= len);
            assert!(target.decl_range.end as usize <= len);
        }
        for (file, symbols) in &snapshot.document_symbols {
            let len = extent(*file);
            for symbol in symbols.iter() {
                assert!(symbol.full_range.end as usize <= len);
            }
        }
    }

    fn charged(facts: &AnalysisFactCollector) -> (u64, u64) {
        match &facts.state {
            FactState::Retaining { count, bytes, .. } => (*count, *bytes),
            FactState::Limited { count, bytes, .. } => (*count, *bytes),
        }
    }

    fn charged_bytes(facts: &AnalysisFactCollector) -> u64 {
        charged(facts).1
    }
}
