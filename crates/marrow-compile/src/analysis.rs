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

/// The largest canonical type display one hover query returns before it is refused as a
/// query-local outcome (never retained in the snapshot). A deeply nested generic exceeds
/// a single interned name yet stays far below the snapshot fact-byte budget.
pub const MAX_HOVER_DISPLAY_BYTES: u64 = 64 * 1024;

/// The largest checked whole-document format output one query returns before it is
/// refused as a query-local outcome (never retained). The formatter's input is already
/// bounded by the pure owner's per-file admission, so this is an expansion guard, not a
/// second input bound.
pub const MAX_FORMAT_OUTPUT_BYTES: u64 = 4 * 1024 * 1024;

/// A fixed analysis resource bound that produced no snapshot. It wraps CRES01's shipped
/// [`CompileResourceLimit`] verbatim for a compile-side aggregate bound, and names the
/// H00f-owned snapshot fact bounds directly. Closed and exhaustively matchable.
pub enum AnalysisResourceLimit {
    /// A compile-side aggregate bound (an image count/byte ceiling, or the CRES01
    /// diagnostic count/byte ceiling on the complete analysis diagnostic set).
    Compile(CompileResourceLimit),
    /// The retained fact count exceeded [`MAX_SNAPSHOT_FACT_COUNT`].
    SnapshotFactCount { limit: u64 },
    /// The retained fact byte footprint exceeded [`MAX_SNAPSHOT_FACT_BYTES`].
    SnapshotFactBytes { limit: u64 },
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

    /// The hover fact at a byte offset in a file: the canonical type display of the
    /// resolved local or parameter use spanning the offset. An unknown file or an
    /// out-of-range offset is a typed [`QueryError`]; a position in a module that did
    /// not parse is [`Unavailability::Syntax`]; a valid position with no fact is
    /// `Absent`.
    pub fn hover(&self, file: &FileIdentity, offset: usize) -> Result<Fact<Hover>, QueryError> {
        let source = self.source_of(file)?;
        if offset > source.len() {
            return Err(QueryError::OffsetOutOfRange);
        }
        if self.broken_files.iter().any(|broken| broken == file) {
            return Ok(Fact::Unavailable(Unavailability::Syntax));
        }
        let offset = offset as u32;
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
    // Enforce the fact publication bounds before retention: an overflow transactionally
    // refuses the whole snapshot as a resource limit rather than admitting a truncated
    // or partial fact set.
    if analysis.hover_facts.len() as u64 > MAX_SNAPSHOT_FACT_COUNT {
        return Err(AnalysisFailure::ResourceLimit {
            revision,
            limit: AnalysisResourceLimit::SnapshotFactCount {
                limit: MAX_SNAPSHOT_FACT_COUNT,
            },
        });
    }
    let fact_bytes: u64 = analysis
        .hover_facts
        .iter()
        .map(|fact| fact.retained_bytes() as u64)
        .sum();
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
    }))
}

/// One retained editor fact: a resolved local or parameter use site and the canonical
/// display of its value type. Held per snapshot and queried by [`AnalysisSnapshot::hover`].
pub(crate) struct HoverFact {
    pub(crate) file: FileIdentity,
    pub(crate) span: marrow_syntax::SourceSpan,
    pub(crate) display: String,
}

impl HoverFact {
    /// The retained byte footprint of this fact: its rendered display. The file
    /// identity and span are fixed-size and charged by the count bound.
    fn retained_bytes(&self) -> usize {
        self.display.len()
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
