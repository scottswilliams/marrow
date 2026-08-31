//! The analysis fact ledger: the one live private owner of the editor facts a snapshot
//! publishes, and the body-local owner a lowering stages its facts in.
//!
//! Split from the analysis module because it is a self-contained substrate with one
//! entry (`absorb`, against a settled body) and one exit (`finish`). The projection that
//! reads it, the queries that answer from it, and the fact shapes it holds stay with
//! their own owners.

use marrow_image::{DraftTxn, ImageDraft};
use marrow_syntax::SourceSpan;

use super::{
    AnalysisFactLimit, BoundedAnalysisFacts, DeclSymbol, DefinitionTarget, FactSpan, FileRef,
    HoverFact, MAX_SNAPSHOT_FACT_BYTES, MAX_SNAPSHOT_FACT_COUNT, RetainedFacts, symbol_bytes,
    symbol_count,
};
use marrow_project::ProjectInput;

use crate::diag::{BoundedDiagnostics, DiagnosticCollector};
use crate::types::{GenericInvariant, GenericOwnerTxn, TypeRegistry};

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
    /// Append one settled body's rows, preserving the order they were produced in: the
    /// body's own rows follow every row already retained, exactly as they would have
    /// landed had the body written straight through.
    fn absorb(&mut self, other: RetainingFacts) {
        let RetainingFacts {
            hover_facts,
            broken_files,
            dependency_gaps,
            document_symbols,
        } = other;
        self.hover_facts.extend(hover_facts);
        self.broken_files.extend(broken_files);
        self.dependency_gaps.extend(dependency_gaps);
        self.document_symbols.extend(document_symbols);
    }

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

    /// Release one settled body's staged facts into this ledger.
    ///
    /// The staged charge composed over exactly these totals while the body ran, so
    /// re-composing it here reaches the same verdict the body already observed: a body
    /// that crossed the ceiling live limits the ledger the moment it settles, and one
    /// that did not cannot.
    pub(crate) fn absorb(&mut self, released: ReleasedFacts) {
        let ReleasedFacts {
            count,
            bytes,
            facts,
        } = released;
        self.admit(count, bytes, move |retained| retained.absorb(facts));
    }

    /// The logical byte charge of one file's spelling. Every coordinate the drive mints
    /// names a module of the project this ledger was built over, so the lookup is total;
    /// an absent one would under-charge silently rather than refuse.
    fn spelling_bytes(&self, file: FileRef) -> u64 {
        // Profiles cannot disagree: `file_bytes` is sized at this ledger's own project
        // and every `FileRef` the drive mints indexes that project, so the `unwrap_or`
        // is unreachable and neither profile ever reads its zero.
        debug_assert!(
            file.index() < self.file_bytes.len(),
            "a coordinate names a module of this ledger's own project"
        );
        self.file_bytes.get(file.index()).copied().unwrap_or(0) as u64
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

    /// The running totals, in either state. A staged body composes its own contribution
    /// over these to reach the same ceiling verdict the ledger would have reached for
    /// the same fact.
    fn totals(&self) -> (u64, u64) {
        match self.state {
            FactState::Retaining { count, bytes, .. } => (count, bytes),
            FactState::Limited { count, bytes, .. } => (count, bytes),
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
                match crossed_ceiling(new_count, new_bytes) {
                    Some(limit) => {
                        self.state = limited_facts(new_count, new_bytes, limit);
                    }
                    None => {
                        *count = new_count;
                        *bytes = new_bytes;
                        retain(facts);
                    }
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

/// Classify one composed total against both ceilings, Count taking precedence over
/// Bytes at a simultaneous crossing.
///
/// The sole owner of the ceiling comparison. The ledger's own admissions and a staged
/// body's live charge both classify through here, so a body cannot reach a different
/// verdict for a fact than the ledger reaches when that fact settles.
fn crossed_ceiling(count: u64, bytes: u64) -> Option<AnalysisFactLimit> {
    if count > MAX_SNAPSHOT_FACT_COUNT {
        Some(AnalysisFactLimit::Count {
            limit: MAX_SNAPSHOT_FACT_COUNT,
        })
    } else if bytes > MAX_SNAPSHOT_FACT_BYTES {
        Some(AnalysisFactLimit::Bytes {
            limit: MAX_SNAPSHOT_FACT_BYTES,
        })
    } else {
        None
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

/// One lowered body's editor facts, held outside every ledger a consumer can reach until
/// the transaction that produced them has committed or run its inverse.
///
/// The split this carries is three-part, and each part is load-bearing.
///
/// The **charge** is live. Every fact composes over the ledger's settled totals at the
/// push that produced it and classifies through [`crossed_ceiling`], the ledger's own
/// comparison, so a body whose facts cross the snapshot ceiling stops rendering displays
/// *inside itself* rather than after it, and no fact population larger than a snapshot
/// admits is ever materialized. The ledger cannot move underneath a running body: this
/// value borrows it shared for the body's whole extent, so the totals a push composes
/// over are the totals release will compose over.
///
/// The **retain** is body-local. The rows and the charge they made live here, not in the
/// ledger, and the private [`Self::finish`] is reachable only through the producer-owning
/// aggregate that releases it.
///
/// The **inverse** is this value's drop, and it is total because it is structural rather
/// than arithmetic: the ledger was never touched, so there is no state in which undoing
/// the charge can fail, and a ceiling an abandoned body crossed cannot latch on a
/// snapshot that body's facts never entered. Subtracting a charge back out could not be
/// total — a crossing discards the ledger's whole retained payload, and no subtraction
/// re-materializes it.
pub(crate) struct StagedFacts {
    /// This body's own contribution, over and above the ledger's settled totals.
    count: u64,
    bytes: u64,
    /// The ceiling this body's own facts crossed, if any. A crossing discards this body's
    /// staged rows at once — the same whole-payload discard the ledger performs — and
    /// stops further rendering, while latching nothing on the ledger until release.
    limit: Option<AnalysisFactLimit>,
    facts: RetainingFacts,
}

/// One settled body's facts on their way into the ledger. Produced only by the private
/// [`StagedFacts::finish`] after the producer-owning aggregate consumes its guard.
pub(crate) struct ReleasedFacts {
    count: u64,
    bytes: u64,
    facts: RetainingFacts,
}

/// One generic-owner producer and both payloads it may publish. The fields never detach:
/// every successful exit consumes this exact producer before sealing its diagnostics and
/// facts, while an invariant or unwind drops the armed producer and both private payloads
/// together.
pub(crate) struct StagedBodyTxn<'r, 'd> {
    owner: GenericOwnerTxn<'r, 'd>,
    diagnostics: DiagnosticCollector,
    facts: StagedFacts,
}

/// The immutable product of one settled body. Its private fields can only be absorbed
/// together, so diagnostics cannot publish without the facts from the same producer or
/// vice versa.
pub(crate) struct ReleasedBody {
    diagnostics: BoundedDiagnostics,
    facts: ReleasedFacts,
}

impl<'r, 'd> StagedBodyTxn<'r, 'd> {
    pub(crate) fn begin(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        Ok(Self::new(GenericOwnerTxn::begin(registry, draft)?))
    }

    pub(crate) fn enter_proof(
        registry: &'r mut TypeRegistry,
        draft: &'d mut ImageDraft,
    ) -> Result<Self, GenericInvariant> {
        Ok(Self::new(GenericOwnerTxn::enter_proof(registry, draft)?))
    }

    fn new(owner: GenericOwnerTxn<'r, 'd>) -> Self {
        Self {
            owner,
            diagnostics: DiagnosticCollector::new(),
            facts: StagedFacts::new(),
        }
    }

    pub(crate) fn parts(
        &mut self,
    ) -> (
        &mut TypeRegistry,
        &mut DraftTxn<'d>,
        &mut DiagnosticCollector,
        &mut StagedFacts,
    ) {
        let Self {
            owner,
            diagnostics,
            facts,
        } = self;
        let (registry, draft) = owner.parts();
        (registry, draft, diagnostics, facts)
    }

    pub(crate) fn registry(&self) -> &TypeRegistry {
        self.owner.registry()
    }

    pub(crate) fn commit(self) -> ReleasedBody {
        let Self {
            owner,
            diagnostics,
            facts,
        } = self;
        owner.commit();
        ReleasedBody {
            diagnostics: diagnostics.finish(),
            facts: facts.finish(),
        }
    }

    pub(crate) fn erase(self) -> ReleasedBody {
        let Self {
            owner,
            diagnostics,
            facts,
        } = self;
        owner.erase();
        ReleasedBody {
            diagnostics: diagnostics.finish(),
            facts: facts.finish(),
        }
    }
}

impl ReleasedBody {
    pub(crate) fn absorb(
        self,
        diagnostics: &mut DiagnosticCollector,
        facts: &mut AnalysisFactCollector,
    ) {
        diagnostics.absorb(self.diagnostics);
        facts.absorb(self.facts);
    }
}

impl StagedFacts {
    fn new() -> Self {
        Self {
            count: 0,
            bytes: 0,
            limit: None,
            facts: RetainingFacts::default(),
        }
    }

    /// Where this body's lowering writes its editor facts, in place of any ledger the
    /// caller owns. The ledger is borrowed shared: a producer can charge against its
    /// totals and cannot retain into it.
    pub(crate) fn sink<'a>(
        &'a mut self,
        ledger: &'a AnalysisFactCollector,
        file: FileRef,
    ) -> FactSink<'a> {
        FactSink::Retaining {
            ledger,
            staged: self,
            file,
        }
    }

    /// Finish this body's facts after its producer has settled. Private so the
    /// producer-owning aggregate below is the only caller that can release them.
    fn finish(self) -> ReleasedFacts {
        let Self {
            count,
            bytes,
            limit: _,
            facts,
        } = self;
        ReleasedFacts {
            count,
            bytes,
            facts,
        }
    }

    /// Whether a fact staged here would still be retained at settlement.
    fn retains(&self, ledger: &AnalysisFactCollector) -> bool {
        !ledger.is_limited() && self.limit.is_none()
    }

    /// Charge one contribution live against the composed total, then stage its payload.
    ///
    /// Crossing discards this body's whole staged payload for the same reason the ledger
    /// discards its own: a crossing refuses the whole snapshot, so there is no partial
    /// population to keep.
    fn admit(
        &mut self,
        ledger: &AnalysisFactCollector,
        added_count: u64,
        added_bytes: u64,
        retain: impl FnOnce(&mut RetainingFacts),
    ) {
        let (settled_count, settled_bytes) = ledger.totals();
        self.count = self.count.saturating_add(added_count);
        self.bytes = self.bytes.saturating_add(added_bytes);
        if self.limit.is_some() {
            return;
        }
        let composed_count = settled_count.saturating_add(self.count);
        let composed_bytes = settled_bytes.saturating_add(self.bytes);
        match crossed_ceiling(composed_count, composed_bytes) {
            Some(limit) => {
                self.limit = Some(limit);
                self.facts = RetainingFacts::default();
            }
            None => retain(&mut self.facts),
        }
    }

    /// Stage one editor hover fact in `file`, charged at the push that produced it.
    fn hover_fact(
        &mut self,
        ledger: &AnalysisFactCollector,
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
        let bytes = fact.retained_bytes(|at| ledger.spelling_bytes(at));
        self.admit(ledger, 1, bytes, move |facts| facts.hover_facts.push(fact));
    }

    /// Stage one dependency gap in `file`. It carries only fixed-size references and a
    /// span, so the count bound charges it and it charges no bytes.
    fn gap_fact(&mut self, ledger: &AnalysisFactCollector, file: FileRef, span: SourceSpan) {
        self.admit(ledger, 1, 0, |facts| {
            facts.dependency_gaps.push((file, FactSpan::of(span)));
        });
    }
}

/// The scoped borrow one body's lowering writes its editor facts through.
///
/// A producer never holds a fact vector of its own: every fact reaches the composed
/// ceilings at the push that produced it, so no single body can stage more facts than a
/// whole snapshot admits. A body whose facts duplicate an already-collected template's is
/// given the `Discarding` state rather than a scratch vector nobody reads.
pub(crate) enum FactSink<'a> {
    Retaining {
        /// Shared: a producer charges against the settled totals and cannot retain into
        /// them.
        ledger: &'a AnalysisFactCollector,
        staged: &'a mut StagedFacts,
        file: FileRef,
    },
    /// This body's facts duplicate a template's, which were collected once at the
    /// template proof. Nothing is retained and nothing is allocated.
    Discarding,
}

impl FactSink<'_> {
    /// Admit one editor hover fact in this sink's file, at the push that produced it.
    /// The composed ceilings charge it before it is staged, so the count a single body
    /// can hold live is bounded by the snapshot ceiling rather than by the body's length.
    pub(crate) fn hover(
        &mut self,
        span: SourceSpan,
        display: Box<str>,
        definition: Option<DefinitionTarget>,
    ) {
        if let FactSink::Retaining {
            ledger,
            staged,
            file,
        } = self
        {
            staged.hover_fact(ledger, *file, span, display, definition);
        }
    }

    /// Stage one dependency gap in this sink's file. Gaps are written as they are
    /// discovered, so one survives an ordinary refusal of the body it sits in.
    pub(crate) fn gap(&mut self, span: SourceSpan) {
        if let FactSink::Retaining {
            ledger,
            staged,
            file,
        } = self
        {
            staged.gap_fact(ledger, *file, span);
        }
    }

    /// Whether a fact written here would still be retained. A producer renders a fact
    /// display only inside this guard: a discarding sink keeps nothing, and once the
    /// ledger is Limited the whole snapshot is already refused, so both are waste.
    pub(crate) fn renders_facts(&self) -> bool {
        match self {
            FactSink::Retaining { ledger, staged, .. } => staged.retains(ledger),
            FactSink::Discarding => false,
        }
    }
}

#[cfg(test)]
mod fact_ledger_tests {
    use super::*;
    // The parent module's own items: the fixtures reach the snapshot shapes and the
    // per-file bounds the ledger is sized against, which live with the projection.
    use super::super::*;
    use crate::SourceDiagnostic;
    use crate::diag::{MAX_DIAGNOSTIC_BYTES, MAX_DIAGNOSTIC_COUNT};
    use marrow_project::{CaptureLimits, CapturedFile, Manifest};
    use std::mem::size_of;
    use std::sync::Arc;

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
    ///
    /// The two per-file `FileRef` lists — `broken_files` and `symbol_bounded_files` —
    /// share the single `max_files()` term at the end. That closes because they are
    /// disjoint by construction, not by coincidence: `broken_files` holds files that did
    /// not parse, and only cleanly-parsed modules are offered to the outline projection
    /// (`compile.rs` filters `!module.broken` before it runs), so no file can appear in
    /// both and their lengths sum to at most one file count.
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
            symbol_bounded_files,
        } = empty_snapshot();
        // Named rather than discarded: this list is the one retained field with no term
        // of its own. It shares `broken_files`' single per-file term, which closes only
        // because the two are disjoint by construction — a file that did not parse is
        // never offered to the outline projection — so a reader checking the term against
        // the field set must see the sharing rather than assume a missing term.
        assert!(
            symbol_bounded_files.is_empty(),
            "the accounting fixture retains nothing",
        );

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
    ///
    /// It is the charge the image crate publishes, not a second copy of the same number:
    /// the factor is a property of how a `Vec` grows, so one owner states it and every
    /// accounting that charges growth reads it there.
    use marrow_image::bounds::GROWTH_AND_COPY;

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

    /// One staged fact payload settled into the ledger. The production wrapper is the
    /// only non-test owner allowed to construct or finish this private payload.
    fn settled_body(
        facts: &mut AnalysisFactCollector,
        file: FileRef,
        body: impl FnOnce(&mut FactSink<'_>),
    ) {
        let mut staged = StagedFacts::new();
        {
            let mut sink = staged.sink(facts, file);
            body(&mut sink);
        }
        facts.absorb(staged.finish());
    }

    /// One private staged payload dropped without settlement.
    fn abandoned_body(
        facts: &AnalysisFactCollector,
        file: FileRef,
        body: impl FnOnce(&mut FactSink<'_>),
    ) {
        let mut staged = StagedFacts::new();
        let mut sink = staged.sink(facts, file);
        body(&mut sink);
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
        settled_body(&mut plain, first(), |sink| {
            sink.hover(span(0, 1), "int".into(), None);
        });
        assert_eq!(charged_bytes(&plain), 3);

        let mut targeted = ledger(&input);
        settled_body(&mut targeted, first(), |sink| {
            sink.hover(
                span(0, 1),
                "int".into(),
                Some(DefinitionTarget::new(first(), span(4, 8), span(0, 20))),
            );
        });
        assert_eq!(charged_bytes(&targeted), 3 + spelling);
    }

    /// A dependency gap carries only fixed-size references and a span, so the count
    /// bound charges it and it charges no bytes.
    #[test]
    fn a_dependency_gap_charges_only_the_count() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        settled_body(&mut facts, first(), |sink| {
            sink.gap(span(0, 4));
        });
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
        settled_body(&mut facts, first(), |sink| {
            for start in 0..16 {
                sink.hover(span(start, start + 1), "int".into(), Some(target));
            }
        });
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
        settled_body(&mut facts, first(), |sink| {
            for index in 0..MAX_SNAPSHOT_FACT_COUNT {
                sink.gap(span(index as usize, index as usize + 1));
            }
        });
        assert!(!facts.is_limited(), "the ceiling itself is admitted");
        settled_body(&mut facts, first(), |sink| sink.gap(span(0, 1)));
        assert!(facts.is_limited());
        // A later admission composes into the limited state; nothing re-materializes.
        settled_body(&mut facts, first(), |sink| {
            sink.hover(span(0, 1), "int".into(), None);
        });
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
        settled_body(&mut facts, first(), |sink| {
            for _ in 0..9 {
                sink.hover(span(0, 1), "x".repeat(chunk).into(), None);
            }
        });
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
        settled_body(&mut facts, first(), |sink| {
            for _ in 0..MAX_SNAPSHOT_FACT_COUNT {
                sink.gap(span(0, 1));
            }
            sink.hover(span(0, 1), display.into(), None);
        });
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
        settled_body(&mut facts, first(), |sink| {
            sink.hover(
                span(0, 1),
                "x".repeat(MAX_SNAPSHOT_FACT_BYTES as usize + 1).into(),
                None,
            );
        });
        assert!(facts.is_limited());
        settled_body(&mut facts, first(), |sink| {
            for index in 0..=MAX_SNAPSHOT_FACT_COUNT {
                sink.gap(span(index as usize, index as usize + 1));
            }
        });
        assert!(matches!(
            facts.finish(),
            BoundedAnalysisFacts::Limited {
                limit: AnalysisFactLimit::Count { .. }
            }
        ));

        let mut reverse = ledger(&input);
        settled_body(&mut reverse, first(), |sink| {
            for index in 0..=MAX_SNAPSHOT_FACT_COUNT {
                sink.gap(span(index as usize, index as usize + 1));
            }
            sink.hover(
                span(0, 1),
                "x".repeat(MAX_SNAPSHOT_FACT_BYTES as usize + 1).into(),
                None,
            );
        });
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

    /// A body that never settles leaves the ledger exactly as it found it.
    ///
    /// This is the inverse, and it is structural rather than arithmetic: the staged owner
    /// holds both the rows and the charge, so dropping it un-charges the body completely.
    /// The comparison is against a ledger that never saw the body at all, so a charge that
    /// leaked through in either direction shows as a terminal difference.
    #[test]
    fn an_abandoned_body_leaves_the_ledger_exactly_as_it_found_it() {
        let input = project(&[("src/main.mw", "")]);
        let target = DefinitionTarget::new(first(), span(4, 8), span(0, 20));

        let mut abandoned = ledger(&input);
        let mut untouched = ledger(&input);
        for facts in [&mut abandoned, &mut untouched] {
            settled_body(facts, first(), |sink| {
                sink.hover(span(0, 1), "int".into(), Some(target));
                sink.gap(span(2, 3));
            });
        }

        // One body writes a wide fact population and is then abandoned: the staged owner
        // is dropped without ever being released.
        abandoned_body(&abandoned, first(), |sink| {
            for index in 0..512 {
                sink.hover(span(index, index + 1), "x".repeat(64).into(), Some(target));
                sink.gap(span(index, index + 1));
            }
        });

        assert_eq!(
            charged(&abandoned),
            charged(&untouched),
            "an abandoned body charges the ledger nothing",
        );
        let (BoundedAnalysisFacts::Complete(abandoned), BoundedAnalysisFacts::Complete(untouched)) =
            (abandoned.finish(), untouched.finish())
        else {
            panic!("both fixtures are far under either ceiling")
        };
        assert_eq!(
            abandoned.hover_facts.len(),
            untouched.hover_facts.len(),
            "an abandoned body retains no hover fact",
        );
        assert_eq!(
            abandoned.dependency_gaps.len(),
            untouched.dependency_gaps.len(),
            "an abandoned body retains no dependency gap",
        );
    }

    /// A body that crossed the ceiling and was then abandoned does not limit the snapshot.
    ///
    /// This is the case an arithmetic un-charge cannot serve. Latching the crossing on the
    /// ledger discards its whole retained payload, and subtracting the abandoned body's
    /// count back out afterwards cannot re-materialize what was discarded — so the ledger
    /// would report a limit, and lose every earlier fact, on account of a body whose facts
    /// never entered the snapshot. Staging the crossing with the body is what makes the
    /// inverse total.
    #[test]
    fn a_body_that_crossed_the_ceiling_and_was_abandoned_limits_nothing() {
        let input = project(&[("src/main.mw", "")]);
        let mut facts = ledger(&input);
        settled_body(&mut facts, first(), |sink| {
            sink.hover(span(0, 1), "int".into(), None);
        });

        abandoned_body(&facts, first(), |sink| {
            for index in 0..=MAX_SNAPSHOT_FACT_COUNT {
                sink.gap(span(index as usize, index as usize + 1));
            }
            assert!(
                !sink.renders_facts(),
                "the staged body observed its own crossing live, inside itself",
            );
        });

        assert!(
            !facts.is_limited(),
            "a ceiling crossed by a body that never settled limits no snapshot",
        );
        let BoundedAnalysisFacts::Complete(retained) = facts.finish() else {
            panic!("the abandoned crossing must not limit the ledger")
        };
        assert_eq!(
            retained.hover_facts.len(),
            1,
            "the fact settled before the abandoned body is still retained",
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
