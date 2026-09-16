//! The analysis fact ledger: the one live private owner of the editor facts a snapshot
//! publishes, and the body-local owner a lowering stages its facts in.
//!
//! A self-contained substrate with one entry (`absorb`, against a settled body) and one
//! exit (`finish`). The projection that reads it, the queries that answer from it, and
//! the fact shapes it holds stay with their own owners.

use marrow_syntax::SourceSpan;

use super::{
    AnalysisFactLimit, BoundedAnalysisFacts, DeclSymbol, DefinitionTarget, FactSpan, FileRef,
    HoverFact, MAX_SNAPSHOT_FACT_BYTES, MAX_SNAPSHOT_FACT_COUNT, RetainedFacts, symbol_bytes,
    symbol_count,
};
use marrow_project::ProjectInput;

use crate::bounded::{Bounded, Ceiling};
use crate::diag::{BoundedDiagnostics, DiagnosticCollector};

mod staging;
pub(crate) use staging::{BodySite, StagedBodyTxn};

/// The one live private analysis-fact owner.
///
/// The structural sibling of the diagnostic collector: every fact is admitted against
/// the typed count and byte ceilings **at the push**, so no fact set larger than a
/// public snapshot bound is ever materialized. Crossing a ceiling discards the whole
/// payload — the incoming fact and every already-admitted one — because a crossing
/// refuses the whole snapshot, leaving no partial publication to unwind.
///
/// Count retains precedence over bytes, and a `Bytes` limit strengthens to `Count`
/// once the composed count crosses; `Count` never weakens.
pub(crate) struct AnalysisFactCollector {
    /// The spelling length of each admitted module, by [`FileRef`]. The ledger owns the
    /// logical byte charges, which are stated over file *spellings* even though no
    /// spelling is stored per fact.
    file_bytes: Vec<u32>,
    state: Bounded<FactCeiling>,
}

/// The snapshot fact ceilings, as the shared bounded owner reads them.
pub(crate) struct FactCeiling;

impl Ceiling for FactCeiling {
    type Payload = RetainingFacts;
    type Limit = AnalysisFactLimit;

    const MAX_COUNT: u64 = MAX_SNAPSHOT_FACT_COUNT;
    const MAX_BYTES: u64 = MAX_SNAPSHOT_FACT_BYTES;

    fn count_limit() -> Self::Limit {
        AnalysisFactLimit::Count {
            limit: MAX_SNAPSHOT_FACT_COUNT,
        }
    }

    fn bytes_limit() -> Self::Limit {
        AnalysisFactLimit::Bytes {
            limit: MAX_SNAPSHOT_FACT_BYTES,
        }
    }

    fn is_bytes(limit: Self::Limit) -> bool {
        matches!(limit, AnalysisFactLimit::Bytes { .. })
    }
}

/// The growable form of the retained set, before `finish` seals it.
#[derive(Default)]
pub(crate) struct RetainingFacts {
    hover_facts: Vec<HoverFact>,
    broken_files: Vec<FileRef>,
    dependency_gaps: Vec<(FileRef, FactSpan)>,
    document_symbols: Vec<(FileRef, Box<[DeclSymbol]>)>,
}

impl RetainingFacts {
    /// Append one settled body's rows after every row already retained, preserving the
    /// order they were produced in.
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
            state: Bounded::new(),
        }
    }

    /// Whether a ceiling has already been crossed. The drive stops rendering fact
    /// displays once this is true — the whole snapshot is already refused, so every
    /// further render is waste. An allocation bound, not a protocol.
    pub(crate) fn is_limited(&self) -> bool {
        self.state.is_limited()
    }

    /// Release one settled body's staged facts into this ledger.
    ///
    /// The staged charge composed over exactly these totals while the body ran, so
    /// re-composing here reaches the verdict the body already observed: a body that
    /// crossed the ceiling live limits the ledger as it settles, and one that did not
    /// cannot.
    fn absorb(&mut self, released: ReleasedFacts) {
        let ReleasedFacts {
            count,
            bytes,
            facts,
        } = released;
        self.state
            .admit((0, 0), count, bytes, move |retained| retained.absorb(facts));
    }

    /// The logical byte charge of one file's spelling. Every coordinate the drive mints
    /// names a module of the project this ledger was built over, so the lookup is total;
    /// an absent one would under-charge silently rather than refuse.
    fn spelling_bytes(&self, file: FileRef) -> u64 {
        // `file_bytes` is sized at this ledger's own project and every `FileRef` the
        // drive mints indexes that project, so the `unwrap_or` is unreachable and no
        // build profile ever reads its zero.
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
        self.state.admit((0, 0), count, bytes, |facts| {
            facts.document_symbols.push((file, symbols));
        });
    }

    /// Record that a module did not parse. Broken-module status is not a public fact
    /// row: it is one coordinate per module, bounded by the same 4096-file admission
    /// limit that bounds the coordinate domain, so it charges neither ceiling.
    pub(crate) fn admit_broken(&mut self, file: FileRef) {
        if let Bounded::Retaining { payload, .. } = &mut self.state {
            payload.broken_files.push(file);
        }
    }

    /// Seal this ledger into its terminal. Total: every state has a terminal.
    pub(crate) fn finish(self) -> BoundedAnalysisFacts {
        match self.state {
            Bounded::Retaining { payload, .. } => BoundedAnalysisFacts::Complete(payload.seal()),
            Bounded::Limited { limit, .. } => BoundedAnalysisFacts::Limited { limit },
        }
    }
}

/// One lowered body's editor facts, held outside every ledger a consumer can reach until
/// the transaction that produced them has committed or run its inverse.
///
/// The **charge** is live: every fact composes over the ledger's settled totals at the
/// push that produced it, so a body whose facts cross the snapshot ceiling stops
/// rendering displays inside itself rather than after it. The ledger is borrowed shared
/// for the body's whole extent, so a push and release compose over the same totals.
///
/// The **retain** is body-local: the rows and the charge they made live here, and the
/// private [`Self::finish`] is reachable only through the producer-owning aggregate.
///
/// The **inverse** is this value's drop, total because it is structural rather than
/// arithmetic: the ledger was never touched, so undoing the charge cannot fail.
/// Subtracting a charge back out could not be total — a crossing discards the ledger's
/// whole retained payload, and no subtraction re-materializes it.
struct StagedFacts(Bounded<FactCeiling>);

/// One settled body's facts on their way into the ledger. Produced only by the private
/// [`StagedFacts::finish`] after the producer-owning aggregate consumes its guard.
struct ReleasedFacts {
    count: u64,
    bytes: u64,
    facts: RetainingFacts,
}

/// The immutable product of one settled body. Its private fields can only be absorbed
/// together, so diagnostics cannot publish without the facts from the same producer or
/// vice versa.
pub(crate) struct ReleasedBody {
    diagnostics: BoundedDiagnostics,
    facts: ReleasedFacts,
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
        Self(Bounded::new())
    }

    /// Where this body's lowering writes its editor facts, in place of any ledger the
    /// caller owns. The ledger is borrowed shared: a producer can charge against its
    /// totals and cannot retain into it.
    fn sink<'a>(&'a mut self, ledger: &'a AnalysisFactCollector, file: FileRef) -> FactSink<'a> {
        FactSink {
            state: FactSinkState::Retaining {
                ledger,
                staged: self,
                file,
            },
        }
    }

    /// Finish this body's facts after its producer has settled. Private so the
    /// producer-owning aggregate below is the only caller that can release them.
    fn finish(self) -> ReleasedFacts {
        let (count, bytes) = self.0.totals();
        let facts = match self.0 {
            Bounded::Retaining { payload, .. } => payload,
            Bounded::Limited { .. } => RetainingFacts::default(),
        };
        ReleasedFacts {
            count,
            bytes,
            facts,
        }
    }

    /// Whether a fact staged here would still be retained at settlement.
    fn retains(&self, ledger: &AnalysisFactCollector) -> bool {
        !ledger.is_limited() && !self.0.is_limited()
    }

    /// Charge one contribution live against the composed total, then stage its payload.
    ///
    /// Crossing discards this body's whole staged payload for the same reason the ledger
    /// discards its own: a crossing refuses the whole snapshot.
    fn admit(
        &mut self,
        ledger: &AnalysisFactCollector,
        added_count: u64,
        added_bytes: u64,
        retain: impl FnOnce(&mut RetainingFacts),
    ) {
        self.0
            .admit(ledger.state.totals(), added_count, added_bytes, retain);
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
/// whole snapshot admits.
pub(crate) struct FactSink<'a> {
    state: FactSinkState<'a>,
}

enum FactSinkState<'a> {
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
    /// A sink for an instance whose facts were already collected at template proof.
    pub(crate) fn discarding() -> Self {
        Self {
            state: FactSinkState::Discarding,
        }
    }

    /// Admit one editor hover fact in this sink's file. The composed ceilings charge it
    /// before it is staged, so what one body holds live is bounded by the snapshot
    /// ceiling rather than by the body's length.
    pub(crate) fn hover(
        &mut self,
        span: SourceSpan,
        display: Box<str>,
        definition: Option<DefinitionTarget>,
    ) {
        if let FactSinkState::Retaining {
            ledger,
            staged,
            file,
        } = &mut self.state
        {
            staged.hover_fact(ledger, *file, span, display, definition);
        }
    }

    /// Stage one dependency gap in this sink's file. Gaps are written as they are
    /// discovered, so one survives an ordinary refusal of the body it sits in.
    pub(crate) fn gap(&mut self, span: SourceSpan) {
        if let FactSinkState::Retaining {
            ledger,
            staged,
            file,
        } = &mut self.state
        {
            staged.gap_fact(ledger, *file, span);
        }
    }

    /// Whether a fact written here would still be retained. A producer renders a fact
    /// display only inside this guard: a discarding sink keeps nothing, and once the
    /// ledger is Limited the whole snapshot is already refused, so both are waste.
    pub(crate) fn renders_facts(&self) -> bool {
        match &self.state {
            FactSinkState::Retaining { ledger, staged, .. } => staged.retains(ledger),
            FactSinkState::Discarding => false,
        }
    }
}

#[cfg(test)]
mod fact_ledger_tests {
    use super::*;
    // The parent module's snapshot shapes and per-file bounds the ledger is sized
    // against, which live with the projection.
    use super::super::*;
    use crate::SourceDiagnostic;
    use crate::diag::{MAX_DIAGNOSTIC_BYTES, MAX_DIAGNOSTIC_COUNT};
    use marrow_project::{CaptureLimits, CapturedFile, Manifest};
    use std::mem::size_of;
    use std::sync::Arc;

    /// The exported term the accounted physical footprint of one live
    /// [`AnalysisSnapshot`] must not exceed — an arithmetic property of the pinned
    /// ceilings and the retained representation, not a runtime check.
    ///
    /// It **excludes** the caller-shared `Arc<ProjectInput>`: its up-to-64 MiB of source
    /// bytes are the caller's charge, shared not copied.
    const MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES: u64 = 12 * 1024 * 1024;

    /// The admitted per-file byte ceiling is inside the retained span coordinate domain,
    /// so every span a snapshot retains round-trips exactly. A widened admission ceiling
    /// must widen [`FactSpan`] with it; this pins the two together.
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
    /// share the single `max_files()` term at the end. That closes only because they are
    /// disjoint by construction: only cleanly-parsed modules are offered to the outline
    /// projection, so their lengths sum to at most one file count.
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

    /// The exact accounted worst case. A change to it is an observable-contract
    /// change, so it is asserted rather than only bounded.
    const ACCOUNTED_WORST_CASE_RETAINED_BYTES: u64 = 11_214_848;

    /// The accounted footprint closes under the exported term.
    ///
    /// Two drift gates keep the accounting honest. The snapshot destructure here is
    /// exhaustive, so a new retained *field* is a build error rather than a silent term
    /// violation; at each retained fact type, `retained_bytes` destructures its own
    /// fields exhaustively, so a new heap-owning field is a build error there rather
    /// than retention no ceiling and no term ever sees.
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
        // Named rather than discarded: this is the one retained field with no term of
        // its own, sharing `broken_files`' per-file term, so a reader checking the terms
        // against the field set sees the sharing rather than a missing term.
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
            "the exact accounted worst case moved; re-derive it before changing \
             this pin"
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
    /// successor: a growing collection is live at three times its admitted length. The
    /// factor is a property of how a `Vec` grows, so one owner states it and every
    /// accounting that charges growth reads it there.
    use marrow_image::bounds::GROWTH_AND_COPY;

    /// The **live fact payload** — everything the ledger holds while it is retaining — is
    /// an arithmetic property of its own ceilings, not a property of the workload that
    /// filled it.
    ///
    /// This keeps `MAX_ANALYSIS_FACT_TRANSIENT_BYTES` from being hostage to fixture
    /// choice. Admission stops at [`MAX_SNAPSHOT_FACT_COUNT`] and
    /// [`MAX_SNAPSHOT_FACT_BYTES`] at the push that produced each fact, so no body,
    /// however wide, can make the payload larger; a denser fixture only reaches the
    /// ceiling sooner. Every charged spelling is held in an exactly sized `Box<str>`, so
    /// the byte ceiling is the physical figure and not only a logical one.
    ///
    /// It does **not** cover the producer-side rendering of one display at a time, built
    /// and freed as it is charged, nor the checker's working set, which is not fact state.
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

    /// The compact retained representation is load-bearing, not cosmetic: the same
    /// accounting against a fact that held owned `FileIdentity` spellings inline — 144
    /// bytes against today's 80, with the spelling charged by neither ceiling — does
    /// **not** close under the exported term.
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
    /// definition target — a logical charge, since no spelling is stored per fact.
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

    /// One admitted count charges exactly one retained fact: a definition target is
    /// carried inside the fact, so the exported term has no second maximum to charge.
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

    /// A body that never settles leaves the ledger exactly as it found it.
    ///
    /// The inverse is structural rather than arithmetic: the staged owner holds both the
    /// rows and the charge, so dropping it un-charges the body completely. The comparison
    /// is against a ledger that never saw the body, so any leak shows as a difference.
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
    /// The case an arithmetic un-charge cannot serve: latching the crossing on the ledger
    /// discards its whole retained payload, and subtracting the abandoned body's count
    /// back out cannot re-materialize it, so the ledger would report a limit and lose
    /// every earlier fact for a body whose facts never entered the snapshot.
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
    /// Agreement with an editor's live document text is a separate owner's obligation.
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
            Bounded::Retaining { count, bytes, .. } | Bounded::Limited { count, bytes, .. } => {
                (*count, *bytes)
            }
        }
    }

    fn charged_bytes(facts: &AnalysisFactCollector) -> u64 {
        charged(facts).1
    }
}
