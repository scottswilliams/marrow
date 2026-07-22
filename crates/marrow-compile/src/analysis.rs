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

use marrow_project::{FileIdentity, ProjectInput};
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
pub struct AnalysisSnapshot {
    input: Arc<ProjectInput>,
    revision: InputRevision,
    diagnostics: Vec<SourceDiagnostic>,
    hover_facts: Vec<HoverFact>,
    /// The identities of input files that did not parse. A hover query in one of these
    /// is [`Unavailability::Syntax`], not `Absent`.
    broken_files: Vec<FileIdentity>,
    /// `(file, callee span)` for qualified calls whose target module did not parse. A
    /// query at one of these positions is [`Unavailability::Dependency`], not `Absent`.
    dependency_gaps: Vec<(FileIdentity, marrow_syntax::SourceSpan)>,
    /// The declaration-hierarchy outline of each cleanly-parsed module file, in source
    /// declaration order. A file that did not parse has no entry — it is in
    /// `broken_files` — and a `document_symbols` query for it is
    /// [`Unavailability::Syntax`], not an absent tree.
    document_symbols: Vec<(FileIdentity, Vec<DeclSymbol>)>,
    /// Every parsed module's tree — cleanly parsed and recovered-broken alike — retained
    /// for the per-query completion re-resolution. A `completions` query re-resolves the
    /// position class and its candidate namespace over these trees per call; it retains no
    /// per-position candidate set. A non-UTF-8 file has no entry, so a completion query in
    /// it is [`Unavailability::Syntax`].
    completion_modules: Vec<CompletionModule>,
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

    /// Resolve the source bytes of an input file, or a typed query error if the file is
    /// not one of the snapshot's analyzed inputs.
    fn source_of(&self, file: &FileIdentity) -> Result<&[u8], QueryError> {
        self.input
            .modules()
            .iter()
            .find(|module| module.identity() == file)
            .map(|module| module.source())
            .ok_or(QueryError::UnknownFile)
    }

    /// Whether an offset falls in a dependency-gap span for `file` — a qualified call
    /// whose target module did not parse, so the fact is unavailable, not absent.
    fn dependency_gap_at(&self, file: &FileIdentity, offset: u32) -> bool {
        self.dependency_gaps.iter().any(|(gap_file, span)| {
            gap_file == file && span.start_byte as u32 <= offset && offset < span.end_byte as u32
        })
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
        let source = self.source_of(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        if self.broken_files.iter().any(|broken| broken == file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        let offset = offset as u32;
        if self.dependency_gap_at(file, offset) {
            return Ok(Fact::Unavailable(Unavailability::Dependency));
        }
        match self.hover_facts.iter().find(|fact| {
            &fact.file == file
                && fact.span.start_byte as u32 <= offset
                && offset < fact.span.end_byte as u32
        }) {
            Some(fact) => Ok(Fact::Present(Hover {
                display: fact.display.clone(),
            })),
            None => Ok(Fact::Absent),
        }
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
        let source = self.source_of(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        if self.broken_files.iter().any(|broken| broken == file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        let offset = offset as u32;
        if self.dependency_gap_at(file, offset) {
            return Ok(Fact::Unavailable(Unavailability::Dependency));
        }
        let target = self.hover_facts.iter().find_map(|fact| {
            if &fact.file == file
                && fact.span.start_byte as u32 <= offset
                && offset < fact.span.end_byte as u32
            {
                fact.definition.as_ref()
            } else {
                None
            }
        });
        match target {
            Some(target) => Ok(Fact::Present(Definition {
                file: target.file.clone(),
                name_span: target.name_span,
                declaration_range: target.decl_range,
            })),
            None => Ok(Fact::Absent),
        }
    }

    /// The checked whole-document format of an input file. Consumes the one
    /// syntax-owned [`marrow_syntax::check_format`] policy — the same the CLI's
    /// `marrow fmt` uses — so the refusal decision is classified once. The output is
    /// bounded by [`MAX_FORMAT_OUTPUT_BYTES`] as a query-local refusal (never retained
    /// in the snapshot). An unknown file is a typed [`QueryError`].
    pub fn format(&self, file: &FileIdentity) -> Result<FormatOutcome, QueryError> {
        let source = self.source_of(file)?;
        let Ok(source) = std::str::from_utf8(source) else {
            // A non-UTF-8 file cannot be lexed; formatting is refused as parse-invalid.
            return Ok(FormatOutcome::Refused(FormatRefusal::ParseInvalid(
                Vec::new(),
            )));
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
        self.source_of(file)?;
        if self.broken_files.iter().any(|broken| broken == file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        match self
            .document_symbols
            .iter()
            .find(|(symbol_file, _)| symbol_file == file)
        {
            Some((_, symbols)) => Ok(Fact::Present(symbols.as_slice())),
            // A validated input that is neither broken nor retained did not parse cleanly;
            // the honest outcome is the same syntax-unavailable verdict, never a fabricated
            // empty tree.
            None => Ok(Fact::Unavailable(Unavailability::Syntax)),
        }
    }

    /// The completion classification and candidate namespace at a byte offset in a file.
    ///
    /// The position class is derived purely positionally from the checker's resolution
    /// model over the retained parse tree — never from the trigger character, document
    /// text, or a token scan. The candidate set is the complete in-scope namespace for the
    /// class: locals and parameters in scope before the offset, module functions, consts,
    /// built-ins, imported module names, and enum type names for an expression name; the
    /// base type's declared fields after `.`/`?.`; an enum's immediate members after `::`;
    /// named types, generic templates, built-in type names, and in-scope type parameters
    /// in a type annotation.
    ///
    /// The set is never prefix-filtered, ranked, or truncated: an over-cap namespace is a
    /// query-local [`CompletionOutcome::Refused`], never a truncated prefix. The
    /// re-resolution is per query over the retained tree and registries — no per-position
    /// candidate set is retained.
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
        let source = self.source_of(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        let Some(module) = self
            .completion_modules
            .iter()
            .find(|module| &module.file == file)
        else {
            // A validated input file with no retained parse tree never parsed (a non-UTF-8
            // file). The honest verdict is syntax-unavailable, never a fabricated empty set.
            return Ok(CompletionOutcome::Ready(Fact::Unavailable(
                Unavailability::Syntax,
            )));
        };
        Ok(completion::resolve(module, offset as u32))
    }

    /// The active-call fact at a byte offset: the innermost enclosing call's callee
    /// signature, its parameter pieces, and the active argument index the offset sits at.
    ///
    /// The enclosing call and active index are derived purely positionally over the
    /// retained parse tree — never from the trigger character or a document-text scan. The
    /// callee resolves to a same-module function or generic template declared in the file;
    /// a generic callee presents its source template signature. The parameter pieces are
    /// separately rendered from the declared spellings so no consumer substring-searches
    /// the signature display, and each piece composes the signature so a consumer can mark
    /// the active one.
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
        let source = self.source_of(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        let Some(module) = self
            .completion_modules
            .iter()
            .find(|module| &module.file == file)
        else {
            // A validated input file with no retained parse tree never parsed (a non-UTF-8
            // file). The honest verdict is syntax-unavailable, never a fabricated absence.
            return Ok(ActiveCallOutcome::Ready(Fact::Unavailable(
                Unavailability::Syntax,
            )));
        };
        Ok(active_call::resolve(module, source, offset as u32))
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
    let analysis = analyze_project(&input);
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
    // A per-file declaration-hierarchy bound overflow transactionally refuses the whole
    // snapshot rather than admitting a partial or truncated outline.
    if let Some(limit) = analysis.symbol_limit {
        let limit = match limit {
            SymbolLimit::Count => AnalysisResourceLimit::DocumentSymbolCount {
                limit: MAX_DOCUMENT_SYMBOLS_PER_FILE,
            },
            SymbolLimit::Depth => AnalysisResourceLimit::DocumentSymbolDepth {
                limit: MAX_SYMBOL_DEPTH,
            },
        };
        return Err(AnalysisFailure::ResourceLimit { revision, limit });
    }
    // Enforce the snapshot fact publication bounds before retention: an overflow
    // transactionally refuses the whole snapshot as a resource limit rather than admitting
    // a truncated or partial fact set. Retained declaration-hierarchy symbols charge the
    // same count and byte bounds as hover/definition facts.
    let retained_symbols: u64 = analysis
        .document_symbols
        .iter()
        .map(|(_, symbols)| symbol_count(symbols))
        .sum();
    if analysis.hover_facts.len() as u64 + retained_symbols > MAX_SNAPSHOT_FACT_COUNT {
        return Err(AnalysisFailure::ResourceLimit {
            revision,
            limit: AnalysisResourceLimit::SnapshotFactCount {
                limit: MAX_SNAPSHOT_FACT_COUNT,
            },
        });
    }
    let symbol_fact_bytes: u64 = analysis
        .document_symbols
        .iter()
        .map(|(file, symbols)| file.as_str().len() as u64 + symbol_bytes(symbols))
        .sum();
    let fact_bytes: u64 = analysis
        .hover_facts
        .iter()
        .map(|fact| fact.retained_bytes() as u64)
        .sum::<u64>()
        + symbol_fact_bytes;
    if fact_bytes > MAX_SNAPSHOT_FACT_BYTES {
        return Err(AnalysisFailure::ResourceLimit {
            revision,
            limit: AnalysisResourceLimit::SnapshotFactBytes {
                limit: MAX_SNAPSHOT_FACT_BYTES,
            },
        });
    }
    Ok(Arc::new(AnalysisSnapshot {
        input,
        revision,
        diagnostics,
        hover_facts: analysis.hover_facts,
        broken_files: analysis.broken_files,
        dependency_gaps: analysis.dependency_gaps,
        document_symbols: analysis.document_symbols,
        completion_modules: analysis.completion_modules,
    }))
}

/// One retained editor fact: a resolved local or parameter use site and the canonical
/// display of its value type. Held per snapshot and queried by [`AnalysisSnapshot::hover`].
pub(crate) struct HoverFact {
    pub(crate) file: FileIdentity,
    pub(crate) span: marrow_syntax::SourceSpan,
    pub(crate) display: String,
    /// The definition target when this fact is a resolved function callee; `None` for a
    /// local or parameter use.
    pub(crate) definition: Option<crate::lower::DefinitionTarget>,
}

impl HoverFact {
    /// The retained byte footprint of this fact: its rendered display plus any retained
    /// definition-target file path. The spans and identities are otherwise fixed-size
    /// and charged by the count bound.
    fn retained_bytes(&self) -> usize {
        self.display.len()
            + self
                .definition
                .as_ref()
                .map_or(0, |target| target.file.as_str().len())
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
    name: String,
    kind: DeclKind,
    name_span: SourceSpan,
    full_range: SourceSpan,
    children: Vec<DeclSymbol>,
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
        self.name_span
    }

    /// The full header-through-body declaration range.
    pub fn full_range(&self) -> SourceSpan {
        self.full_range
    }

    /// The nested member children, in source order.
    pub fn children(&self) -> &[DeclSymbol] {
        &self.children
    }

    /// This node's retained byte footprint: its name spelling. Spans and the kind are
    /// fixed-size and charged by the count bound. Children are summed separately.
    fn retained_bytes(&self) -> u64 {
        self.name.len() as u64
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
) -> Result<Vec<DeclSymbol>, SymbolLimit> {
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
        let leaf = |name: String, kind: DeclKind, name_span: SourceSpan, full_range: SourceSpan| {
            DeclSymbol {
                name,
                kind,
                name_span,
                full_range,
                children: Vec::new(),
            }
        };
        let symbol = match declaration {
            Declaration::Alias(alias) => leaf(
                alias.name.clone(),
                DeclKind::Alias,
                alias.name_span,
                alias.span,
            ),
            Declaration::Nominal(nominal) => leaf(
                nominal.name.clone(),
                DeclKind::Nominal,
                nominal.name_span,
                nominal.span,
            ),
            // A `const` declaration carries no separate name span in the AST, so its
            // selection range is its full declaration range.
            Declaration::Const(konst) => {
                leaf(konst.name.clone(), DeclKind::Const, konst.span, konst.span)
            }
            Declaration::Resource(resource) => leaf(
                resource.name.clone(),
                DeclKind::Resource,
                resource.name_span,
                resource.span,
            ),
            Declaration::Struct(item) => leaf(
                item.name.clone(),
                DeclKind::Struct,
                item.name_span,
                item.span,
            ),
            // A store's declared name is its saved-root spelling; its name span covers
            // the `^root` sigiled root.
            Declaration::Store(store) => leaf(
                store.root.root.clone(),
                DeclKind::Store,
                store.root.span,
                store.span,
            ),
            Declaration::Function(function) => leaf(
                function.name.clone(),
                DeclKind::Function,
                function.name_span,
                function.span,
            ),
            Declaration::Test(test) => {
                leaf(test.name.clone(), DeclKind::Test, test.name_span, test.span)
            }
            Declaration::Enum(item) => {
                let children = self.members(&item.members, depth + 1)?;
                DeclSymbol {
                    name: item.name.clone(),
                    kind: DeclKind::Enum,
                    name_span: item.name_span,
                    full_range: item.span,
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
    ) -> Result<Vec<DeclSymbol>, SymbolLimit> {
        members
            .iter()
            .map(|member| self.member(member, depth))
            .collect()
    }

    fn member(&mut self, member: &EnumMember, depth: u16) -> Result<DeclSymbol, SymbolLimit> {
        self.admit(depth)?;
        let children = self.members(&member.members, depth + 1)?;
        Ok(DeclSymbol {
            name: member.name.clone(),
            kind: DeclKind::EnumMember,
            name_span: member.name_span,
            full_range: member.span,
            children,
        })
    }
}

/// One parsed module's tree retained for the per-query completion re-resolution. Every
/// module that produced a parse tree — cleanly parsed and recovered-broken alike — has
/// one; a non-UTF-8 file that never parsed has none.
pub(crate) struct CompletionModule {
    pub(crate) file: FileIdentity,
    pub(crate) ast: marrow_syntax::SourceFile,
}

/// The closed set of completion position classes, derived purely positionally from the
/// checker's resolution model over the retained parse tree — never from the trigger
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
/// This is a distinct read-only pass over the retained parse tree. It never drives the
/// compile-path lowerer or resolver — whose arms assume post-`has_errors` input and can
/// raise resolution invariants — so it runs safely on a broken file and leaks no
/// diagnostic. A partial or unresolvable base yields an empty candidate set; the position
/// class is derived purely positionally.
mod completion {
    use marrow_syntax::{
        Block, Declaration, EnumDecl, EnumMember, Expression, FunctionDecl, Recovery,
        ResourceMember, SourceFile, SourceSpan, Statement, TypeExpr,
    };

    use crate::lower::builtin_value_names;
    use crate::scalar::ScalarType;

    use super::{
        AnalysisResourceLimit, Candidate, CandidateKind, CompletionModule, CompletionOutcome,
        Completions, Fact, MAX_COMPLETION_CANDIDATES, MAX_COMPLETION_RENDER_BYTES, PositionClass,
    };

    /// One in-scope binding: its spelling and, when annotated, its declared type node
    /// borrowed from the retained tree. The type node is the fail-soft type-probe: a
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

    /// Classify the offset over a module's retained tree and enumerate the class namespace.
    pub(super) fn resolve(module: &CompletionModule, offset: u32) -> CompletionOutcome {
        let file = &module.ast;
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
                ty: ty.as_ref(),
            }),
            Statement::PlaceBinding { name, .. } => Some(Binding {
                name: name.clone(),
                ty: None,
            }),
            Statement::LetElse { name, ty, .. } => Some(Binding {
                name: name.clone(),
                ty: ty.as_ref(),
            }),
            Statement::Checked { bind, .. } => match bind {
                marrow_syntax::CheckedBind::Const { name, ty }
                | marrow_syntax::CheckedBind::Var { name, ty } => Some(Binding {
                    name: name.clone(),
                    ty: ty.as_ref(),
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
                        ty: ty.as_ref(),
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
            let segment = use_decl
                .name
                .rsplit("::")
                .next()
                .unwrap_or(use_decl.name.as_str());
            candidates.push(Candidate {
                label: segment.to_string(),
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
        let [name] = segments.as_slice() else {
            return None;
        };
        let binding = scope
            .locals
            .iter()
            .rev()
            .chain(scope.params.iter())
            .find(|binding| &binding.name == name)?;
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
                Declaration::Enum(item) if &item.name == enum_name => Some(item),
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
    fn resolve_enum_members<'a>(item: &'a EnumDecl, rest: &[String]) -> Option<&'a [EnumMember]> {
        let mut members = item.members.as_slice();
        for segment in rest {
            let member = members.iter().find(|member| &member.name == segment)?;
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
/// Like [`completion`], a distinct read-only pass over the retained parse tree: it never
/// drives the compile-path resolver, so it runs on a broken file over recovered
/// incomplete-call nodes and leaks no diagnostic. It collects the calls in the enclosing
/// declaration, selects the innermost whose argument region holds the offset, resolves the
/// callee to a same-module function or generic template in the retained tree, renders the
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
        ActiveCall, ActiveCallOutcome, AnalysisResourceLimit, CompletionModule, Fact,
        MAX_ACTIVE_CALL_RENDER_BYTES, ParamPiece,
    };

    /// One call node reached during collection: its callee expression, its arguments
    /// parsed so far, and its full span. A recovered incomplete call carries the arguments
    /// parsed before the missing delimiter and a span ending at the last parsed token.
    struct CallSite<'a> {
        callee: &'a Expression,
        args: &'a [Argument],
        span: SourceSpan,
    }

    pub(super) fn resolve(
        module: &CompletionModule,
        source: &[u8],
        offset: u32,
    ) -> ActiveCallOutcome {
        let file = &module.ast;
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

    /// Resolve a callee expression to a same-module function or generic template in the
    /// retained tree. A qualified (cross-module) name, a built-in, or an unknown name
    /// resolves to no local declaration on this floor.
    fn resolve_callee<'a>(file: &'a SourceFile, callee: &Expression) -> Option<&'a FunctionDecl> {
        let Expression::Name { segments, .. } = callee else {
            return None;
        };
        let [name] = segments.as_slice() else {
            return None;
        };
        file.declarations
            .iter()
            .find_map(|declaration| match declaration {
                Declaration::Function(function) if &function.name == name => Some(function),
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
