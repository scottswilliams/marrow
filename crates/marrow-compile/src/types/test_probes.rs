//! Test-only observation probes for the type registry — build counters and scaling
//! tallies the scaling tests read. A probe cannot alter registry state or make a
//! hostile state reachable.

use super::*;

thread_local! {
    pub(crate) static METADATA_DIRECTORY_BUILDS: Cell<usize> = const { Cell::new(0) };
    pub(crate) static READY_BODY_MATCH_VISITS: Cell<usize> = const { Cell::new(0) };
}

struct MetadataBuildCounter {
    pub(crate) previous: usize,
}

impl Drop for MetadataBuildCounter {
    fn drop(&mut self) {
        METADATA_DIRECTORY_BUILDS.with(|count| count.set(self.previous));
    }
}

/// Observe directory construction in one single-threaded production journey. This
/// test-only counter cannot alter registry state or make a hostile state reachable.
pub(crate) fn count_metadata_directory_builds<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let previous = METADATA_DIRECTORY_BUILDS.with(|count| count.replace(0));
    let guard = MetadataBuildCounter { previous };
    let result = run();
    let builds = METADATA_DIRECTORY_BUILDS.with(Cell::get);
    drop(guard);
    (result, builds)
}

struct ReadyBodyMatchCounter {
    pub(crate) previous: usize,
}

impl Drop for ReadyBodyMatchCounter {
    fn drop(&mut self) {
        READY_BODY_MATCH_VISITS.with(|count| count.set(self.previous));
    }
}

/// Count borrowed template-body matcher frames in one test journey. The counter
/// observes work only; it cannot alter metadata or make a hostile row reachable.
pub(super) fn count_ready_body_match_visits<T>(run: impl FnOnce() -> T) -> (T, usize) {
    let previous = READY_BODY_MATCH_VISITS.with(|count| count.replace(0));
    let guard = ReadyBodyMatchCounter { previous };
    let result = run();
    let visits = READY_BODY_MATCH_VISITS.with(Cell::get);
    drop(guard);
    (result, visits)
}

/// Deterministic operation counts for the generic-scaling KATs. Every field counts
/// work performed by one production owner during a single-threaded test journey;
/// the counter observes work only and cannot alter registry state or make a hostile
/// row reachable. It is not a public hook and not a canonical fact.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct ScalingCounts {
    /// `MetadataScratch::try_new` invocations (directory builds).
    pub(crate) directory_builds: usize,
    /// Generic instantiation rows classified into directories, counting a full build's
    /// whole population and an incremental extension's newly appended rows alike. When a
    /// directory is rebuilt per probe this is `directory_builds * type_insts.len()`; when
    /// the reused row directory is extended it is one visit per appended row, so it grows
    /// linearly with the instantiation count rather than quadratically.
    pub(crate) directory_row_visits: usize,
    /// Elements examined by the `(template, args)` primary-key scan in
    /// `existing_type_instance` (type-mint reuse).
    pub(crate) type_inst_scan_steps: usize,
    /// Elements examined by the `(template, args)` primary-key scan in
    /// `reserve_fn_instance` (function-mint reuse).
    pub(crate) fn_inst_scan_steps: usize,
    /// Reuse probes performed by `instantiate_collection` (collection-mint dedup). With
    /// the keyed index this is one keyed lookup per instantiation attempt, so the count is
    /// the mint-attempt count and grows linearly with the collection population — the
    /// former linear spec scan made per-attempt work O(collections), i.e. O(collections²).
    pub(crate) coll_inst_probe_steps: usize,
    /// Value-graph edges traversed across every `cycle_through` start.
    pub(crate) cycle_walk_steps: usize,
    /// `enter_template_proof` admissions (one isolated proof pass per generic template).
    pub(crate) proof_clones: usize,
    /// Type-inst rows the proof pass classifies into the shared metadata directory — the
    /// rows its own body mints, extended onto the already-built population directory rather
    /// than replayed over the whole settled population. Constant per template, decoupled from
    /// the instantiation count.
    pub(crate) proof_clone_rows: usize,
    /// Characters rendered into editor hover displays across the whole compile
    /// (`ty.spelling`/signature displays). A monomorphized instance body's facts are
    /// discarded — an instance's use-site spans duplicate its template's — so its
    /// spelling is never rendered; only monomorphic function and test bodies contribute.
    /// On a divergent-monomorphization program the pre-repair per-instance render made
    /// this Σ O(depth) = O(instances²); the repair holds it to the monomorphic baseline.
    pub(crate) hover_spelling_chars: usize,
    /// Template-body declaration entries a fill copied out of the template — one per
    /// declared field for a struct fill, and one per declared variant plus one per
    /// declared payload leaf for an enum fill.
    ///
    /// This counts the copy itself rather than the process footprint around it. The
    /// issuance RSS gate measures an aggregate resident peak, which cannot attribute a
    /// figure to this term; a corpus can also stop on the mint-depth bound long before the
    /// instantiation ceiling, so "driven to the ceiling" is not a safe proxy for the
    /// number of copies either. The exact figure is pinned by
    /// `a_fill_copies_no_template_body_entries`.
    pub(crate) template_body_clone_entries: usize,
}

thread_local! {
    pub(crate) static SCALING_COUNTS: Cell<ScalingCounts> = const { Cell::new(ScalingCounts {
        directory_builds: 0,
        directory_row_visits: 0,
        type_inst_scan_steps: 0,
        fn_inst_scan_steps: 0,
        coll_inst_probe_steps: 0,
        cycle_walk_steps: 0,
        proof_clones: 0,
        proof_clone_rows: 0,
        hover_spelling_chars: 0,
        template_body_clone_entries: 0,
    }) };
}

/// Observe editor hover-display rendering work: the character length of one rendered
/// hover display. A no-op outside the scaling-count test window.
pub(crate) fn bump_hover_spelling_chars(chars: usize) {
    bump_scaling(|counts| counts.hover_spelling_chars += chars);
}

pub(super) fn bump_scaling(update: impl FnOnce(&mut ScalingCounts)) {
    SCALING_COUNTS.with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

/// Run `run` with a fresh scaling-count window and restore the prior window after,
/// returning its result paired with the deterministic operation counts observed.
pub(crate) fn capture_scaling_counts<T>(run: impl FnOnce() -> T) -> (T, ScalingCounts) {
    let previous = SCALING_COUNTS.with(|cell| cell.replace(ScalingCounts::default()));
    let result = run();
    let counts = SCALING_COUNTS.with(Cell::get);
    SCALING_COUNTS.with(|cell| cell.set(previous));
    (result, counts)
}

/// Deterministic work counts for alias-cycle classification. These observe the
/// real alias-table owner only in ordinary test builds and cannot affect graph
/// state, diagnostics, or accepted programs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct AliasCycleCounts {
    pub(crate) target_visits: usize,
    pub(crate) resolved_edges: usize,
    pub(crate) node_entries: usize,
    pub(crate) edge_inspections: usize,
    pub(crate) cyclic_aliases: usize,
}

thread_local! {
    pub(crate) static ALIAS_CYCLE_COUNTS: Cell<AliasCycleCounts> = const {
        Cell::new(AliasCycleCounts {
            target_visits: 0,
            resolved_edges: 0,
            node_entries: 0,
            edge_inspections: 0,
            cyclic_aliases: 0,
        })
    };
}

pub(super) fn bump_alias_cycle(update: impl FnOnce(&mut AliasCycleCounts)) {
    ALIAS_CYCLE_COUNTS.with(|cell| {
        let mut counts = cell.get();
        update(&mut counts);
        cell.set(counts);
    });
}

pub(super) fn capture_alias_cycle_counts<T>(run: impl FnOnce() -> T) -> (T, AliasCycleCounts) {
    let previous = ALIAS_CYCLE_COUNTS.with(|cell| cell.replace(AliasCycleCounts::default()));
    let result = run();
    let counts = ALIAS_CYCLE_COUNTS.with(Cell::get);
    ALIAS_CYCLE_COUNTS.with(|cell| cell.set(previous));
    (result, counts)
}

/// Count the declaration entries one fill copied out of a template body.
///
/// The distinction is ownership, not a claim the caller makes about itself: a fill that
/// reads the body through a handle the template still holds copies nothing, and one
/// holding the only handle to those entries owns a private copy of all of them. A fill
/// that stops sharing therefore charges itself here without the counting call changing.
pub(super) fn count_template_body_copy<T>(body: &Rc<[T]>, entries: impl FnOnce(&[T]) -> usize) {
    let copied = if Rc::strong_count(body) == 1 {
        entries(body)
    } else {
        0
    };
    bump_scaling(|counts| counts.template_body_clone_entries += copied);
}

/// The declaration entries an enum template body carries: one per declared variant plus
/// one per declared payload leaf.
pub(super) fn variant_entries(variants: &[TemplateVariant]) -> usize {
    variants.len()
        + variants
            .iter()
            .map(|variant| variant.payload.len())
            .sum::<usize>()
}
