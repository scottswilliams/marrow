//! DeclarationSite-name ledgers: the one way a declared key leaves the accepted set.
//!
//! A namespace that refuses a declaration must not forget it. Dropping the key
//! makes every later lookup read as *never declared*, so the compiler reports a
//! fabricated absence at each use instead of the cause it already diagnosed at the
//! declaration. A ledger keeps the refused key with a summary of its first refusal,
//! so a lookup answers `Refused` — carrying the declaring code, file, and span —
//! rather than `Absent`.
//!
//! The retained summary is the only owned retention this module adds, and it is
//! charged at `declare` against the pass's one [`DeclarationBudget`], whose ceiling
//! is [`MAX_DECLARATION_LEDGER_BYTES`]. The charge is per refused *key*: re-refusing
//! a key already in the ledger increments a bounded occurrence count and costs
//! nothing, which is what holds amplification to the number of refused declarations
//! rather than the number of uses.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use marrow_codes::Code;
use marrow_project::FileIdentity;
use marrow_syntax::SourceSpan;

use crate::analysis::FileRef;
use crate::diag::{DiagnosticCollector, IdentityGap, MAX_DIAGNOSTIC_BYTES, SourceDiagnostic};

/// The most owned bytes every declaration ledger in one pass may retain together.
///
/// DeclarationSite, not derived from a length: the ledger is live concurrently with the
/// diagnostic collector, and no retained refusal is worth more than the report that
/// accompanies it, so the ledger's budget is the collector's. The refused-key count
/// is otherwise bounded only by the admitted source (`CaptureLimits::DEFAULT`
/// admits up to 64 MiB), because a refused declaration reaches neither the image
/// bounds nor a halt at the diagnostic ceiling — a `Limited` collector keeps
/// admitting and discarding while the pass runs on.
pub(crate) const MAX_DECLARATION_LEDGER_BYTES: usize = MAX_DIAGNOSTIC_BYTES;

/// The ledger's byte budget is spent; the pass stops with a typed resource limit
/// rather than dropping a key and fabricating an absence at its uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclarationLedgerFull;

/// The one retention budget every declaration ledger of a pass charges against.
///
/// One budget, not one per namespace: [`MAX_DECLARATION_LEDGER_BYTES`] bounds what
/// the pass retains while the diagnostic collector is live, and a pass runs six
/// production ledgers, so a per-ledger charge would admit six times the declared
/// bound. Shared by handle rather than by `&mut`, because the ledgers are owned by
/// separate registries that are built in sequence and then read together — no
/// single mutable borrow spans them. A semantic pass runs on one thread, and the
/// handle is deliberately not `Sync`.
#[derive(Clone, Default)]
pub(crate) struct DeclarationBudget(Rc<Cell<usize>>);

impl DeclarationBudget {
    /// Charge `bytes` against the pass's remaining budget, or report the ceiling.
    fn charge(&self, bytes: usize) -> Result<(), DeclarationLedgerFull> {
        let charged = self.0.get().saturating_add(bytes);
        if charged > MAX_DECLARATION_LEDGER_BYTES {
            return Err(DeclarationLedgerFull);
        }
        self.0.set(charged);
        Ok(())
    }

    #[cfg(test)]
    fn spent(&self) -> usize {
        self.0.get()
    }
}

/// Which namespace's ledger minted a [`DeclarationRefusalId`].
///
/// The tag is what makes an id comparable across ledgers. Several namespaces
/// answer one resolution — a type name, a generic template name, and a store root
/// name all reach `ResolveRefusal` — so a bare index would let two unrelated
/// refusals with equal indexes compare equal and collapse into one steer. Every
/// variant names a live producer; a namespace earns a variant when its ledger
/// lands, never before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum DeclarationNamespace {
    Constant,
    DurableRoot,
    /// One function signature per `(module, name)`. A signature refused for a
    /// parameter or return type keeps its name, so a call reuses that cause instead
    /// of reading the project's whole call table as withheld.
    Function,
    /// The project's importable modules, keyed by dotted path. A module is refused
    /// when its header disagrees with its path, and when the stage that produced its
    /// source refused it outright.
    Module,
    NamedType,
    /// The members of one resource record or one of its unkeyed groups, keyed by
    /// the owner they are written in. A member is the one declaration this line
    /// refuses *without* refusing what contains it, so the containing record
    /// survives with a narrowed member set and every lookup of the refused member
    /// would otherwise read as never written.
    ResourceMember,
}

/// A `Copy` handle to one refused declaration, valid only in the ledger that
/// minted it. Refusal causes travel through `ResolveRefusal` as this id so that
/// no owned bytes enter the monomorphization cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DeclarationRefusalId {
    namespace: DeclarationNamespace,
    index: u32,
}

impl DeclarationRefusalId {
    /// The ledger that answers this id.
    pub(crate) fn namespace(self) -> DeclarationNamespace {
        self.namespace
    }
}

/// Why one declared key is refused: the first refusal's cause, plus a bounded
/// count of the further occurrences merged into it.
///
/// The declared name is retained because no consumer can render it otherwise —
/// `reject_resolution` takes a subject *phrase* ("this parameter type"), never the
/// name, and on the rejected-instantiation replay path the name is structurally
/// gone. The retention is charged, not denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeclarationRefusalSummary {
    name: String,
    code: &'static str,
    further: u16,
    gap: Option<IdentityGap>,
    report: RefusalReport,
    /// Whether a use site has already been steered to this cause. A `Cell` because
    /// the flag is the report-once record and every namespace ledger is read through
    /// a shared reference during lowering; the alternative is the parallel
    /// `&mut BTreeSet<String>` of steered names this ledger replaces.
    steered: Cell<bool>,
}

/// Where the report that carries a refusal's cause was made.
///
/// A steer sends the reader to the cause, so it must not claim a location the
/// report does not occupy. Most refusals report at the declaration itself and can
/// say so; the covered classes are reported by another pass or another occurrence,
/// where only the code is known here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusalReport {
    /// The refusing site pushed the row at this declaration's own span.
    AtDeclaration,
    /// A different pass or a different occurrence made the report. The steer names
    /// the code and makes no claim about where the row sits.
    ByCoveringPass,
    /// A stage that ran before the semantic pass refused the whole source this
    /// declaration was written in, and already reported why. The steer names that
    /// stage, because the reader looks for the report in the refused source rather
    /// than at the declaration that names it.
    ByEarlierStage(SourceStage),
}

/// A stage that produces module source before the semantic pass reads it, with the
/// diagnostic code it refuses a whole module with.
///
/// One owner for both facts: a caller names the stage, never the code, so a retained
/// cause cannot cite a report that stage does not make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceStage {
    /// The file's bytes are not UTF-8, so it never entered parsing.
    Decode,
    /// The file is not well-formed Marrow.
    Parse,
}

impl SourceStage {
    /// The code this stage refuses a whole module with.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Decode => Code::CheckUnsupported.as_str(),
            Self::Parse => Code::ParseSyntax.as_str(),
        }
    }

    /// How the steer names this stage's work, as the reader met it.
    fn past_participle(self) -> &'static str {
        match self {
            Self::Decode => "read",
            Self::Parse => "parsed",
        }
    }
}

/// Push the diagnostic that refuses `name` and summarize it from the same triple,
/// so a retained refusal cannot describe a report that was never made.
///
/// This is the only way a [`DeclarationRefusalSummary`] is built outside the
/// ledger's own merge, which is what makes "push the diagnostic and `continue`"
/// inexpressible against a ledger: the pushed row and the retained summary are one
/// statement.
pub(crate) fn refuse(
    diagnostics: &mut DiagnosticCollector,
    at: DeclarationSite<'_>,
    code: &'static str,
    message: String,
) -> DeclarationRefusalSummary {
    refuse_row(
        diagnostics,
        at,
        SourceDiagnostic::at(code, at.file, at.span, message),
    )
}

/// The same coupling for a refusal whose row a shared renderer already built: the
/// summary's code is read off the row that is pushed in the same statement, never
/// restated by the caller.
pub(crate) fn refuse_row(
    diagnostics: &mut DiagnosticCollector,
    at: DeclarationSite<'_>,
    row: SourceDiagnostic,
) -> DeclarationRefusalSummary {
    let summary = covered(at, row.code(), RefusalReport::AtDeclaration);
    diagnostics.push(row);
    summary
}

/// Push a refusal row, keeping the first one as the declaration's retained cause.
///
/// A declaration is refused whole for a defect in any member, and every offending
/// member still gets its own report. The summary carries the first, so the use
/// site is steered to the first thing the reader has to fix rather than to an
/// arbitrary later one.
pub(crate) fn refuse_first(
    refusal: &mut Option<DeclarationRefusalSummary>,
    diagnostics: &mut DiagnosticCollector,
    at: DeclarationSite<'_>,
    row: SourceDiagnostic,
) {
    match refusal {
        Some(_) => diagnostics.push(row),
        None => *refusal = Some(refuse_row(diagnostics, at, row)),
    }
}

/// The same coupling for a refusal whose report is made by a *different* pass or a
/// *different* occurrence, named by the caller.
///
/// Two shapes need it, and no third may: a cause the site cannot report because the
/// pass that owns it runs later, and a cause an earlier occurrence of the same
/// project-wide anchor already reported. Both are on the absence gate's allowlist
/// with the covering report named, so this constructor cannot become the way a
/// refusal escapes reporting altogether.
pub(crate) fn refuse_covered(
    at: DeclarationSite<'_>,
    code: &'static str,
) -> DeclarationRefusalSummary {
    covered(at, code, RefusalReport::ByCoveringPass)
}

/// The same coupling for a declaration whose whole source an earlier stage refused
/// and reported.
///
/// The stage owns the code, so this cannot cite a report the named stage does not
/// make. Re-deriving the stage's own row here would either double-report it or
/// re-plumb that stage to hand its rows back for nothing: the row already stands in
/// the terminal the reader is shown.
pub(crate) fn refuse_at_earlier_stage(
    at: DeclarationSite<'_>,
    stage: SourceStage,
) -> DeclarationRefusalSummary {
    covered(at, stage.code(), RefusalReport::ByEarlierStage(stage))
}

fn covered(
    at: DeclarationSite<'_>,
    code: &'static str,
    report: RefusalReport,
) -> DeclarationRefusalSummary {
    DeclarationRefusalSummary {
        name: at.name.to_string(),
        code,
        further: 0,
        gap: None,
        report,
        steered: Cell::new(false),
    }
}

/// The row that steers one use of a refused declaration to the cause its
/// declaration reported.
///
/// The row carries the *declaring* code, so a use-site assertion names the
/// declaration's typed identity and the reader follows one code to one fix.
pub(crate) fn declaration_refused(
    file: &FileIdentity,
    span: SourceSpan,
    refusal: &DeclarationRefusalSummary,
) -> SourceDiagnostic {
    let name = refusal.name();
    SourceDiagnostic::at(
        refusal.code(),
        file,
        span,
        format!(
            "`{name}` was declared, but its declaration was refused. A refused \
             declaration keeps its name and binds no value, so this use cannot \
             resolve. {}",
            refusal.correction()
        ),
    )
}

/// Where one declaration is written: its name, its file in both the owned spelling
/// a diagnostic renders and the `Copy` coordinate a summary retains, and its span.
#[derive(Clone, Copy)]
pub(crate) struct DeclarationSite<'a> {
    pub(crate) name: &'a str,
    pub(crate) file: &'a FileIdentity,
    pub(crate) at: FileRef,
    pub(crate) span: SourceSpan,
}

impl<'a> DeclarationSite<'a> {
    /// A declaration whose whole source an earlier stage refused, so it has no span
    /// of its own: a file that did not decode or did not parse produced no construct
    /// to point at, and the report the reader follows is that stage's.
    pub(crate) fn whole_file(name: &'a str, file: &'a FileIdentity, at: FileRef) -> Self {
        Self {
            name,
            file,
            at,
            span: SourceSpan {
                start_byte: 0,
                end_byte: 0,
                line: 1,
                column: 1,
            },
        }
    }
}

impl DeclarationRefusalSummary {
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The declaring diagnostic's stable code, which the causal steer reuses so a
    /// use-site assertion carries the declaration's typed identity.
    pub(crate) fn code(&self) -> &'static str {
        self.code
    }

    /// Attach the identity gap this refusal carries.
    ///
    /// Only the identity class does: its steer sends the reader to the
    /// `check.durable_identity` report family rather than to a single declaring
    /// row, so the gap is what tells the two classes apart at the use site. The
    /// retained path is a second copy of one the collector already holds, and §4's
    /// term charges it as such — which is why no other class carries one.
    pub(crate) fn with_gap(mut self, gap: IdentityGap) -> Self {
        self.gap = Some(gap);
        self
    }

    /// The identity gap this refusal carries, if it is the identity class.
    pub(crate) fn gap(&self) -> Option<&IdentityGap> {
        self.gap.as_ref()
    }

    /// What the reader has to correct, phrased so it names a location only where a
    /// report actually sits.
    ///
    /// A steer sends the reader to the cause. Most refusals report at the
    /// declaration itself and can say so; a covered class — a value cycle the later
    /// cycle pass reports, an anchor an earlier occurrence reported, a source an
    /// earlier stage refused whole — has its row somewhere else entirely, and
    /// claiming it sits at this declaration would send the reader to a row that is
    /// not there.
    pub(crate) fn correction(&self) -> String {
        let (name, code) = (self.name(), self.code());
        match self.report {
            RefusalReport::AtDeclaration => {
                format!("Correct the `{code}` report at the declaration of `{name}`.")
            }
            RefusalReport::ByCoveringPass => format!("Correct the reported `{code}`."),
            RefusalReport::ByEarlierStage(stage) => format!(
                "Correct the `{code}` reports `{name}` received when it was {}.",
                stage.past_participle()
            ),
        }
    }

    /// Whether this use site is the one that reports the cause. `true` on the first
    /// call only, so many uses of one refused key report it once and fail silently
    /// thereafter — the property that holds amplification to the number of refused
    /// declarations rather than the number of uses.
    pub(crate) fn steer_once(&self) -> bool {
        !self.steered.replace(true)
    }

    /// The owned bytes this summary retains: the declared name, the optional gap
    /// path, and the fixed footprint. Every other field is `Copy`.
    fn retained_owned_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.name.len()
            + self.gap.as_ref().map_or(0, |gap| gap.path.len())
    }

    /// Merge a further refusal of the same key into this summary. Exhaustive by
    /// destructure so a new field cannot be silently left unmerged.
    fn merge(&mut self, other: Self) {
        let Self {
            name: _,
            code: _,
            further,
            gap,
            report: _,
            steered: _,
        } = other;
        // The first refusal is the reported cause; a later one only raises the
        // count. A gap arriving with a later occurrence still completes the
        // identity class, which renders from the gap rather than the code.
        self.further = self.further.saturating_add(further).saturating_add(1);
        if self.gap.is_none() {
            self.gap = gap;
        }
    }
}

/// One occurrence of a declared key: the accepted value, or why it was refused.
///
/// `declare` takes this by value, so "push the diagnostic and `continue`" is not
/// expressible against a ledger — the refusal has to be handed over.
#[derive(Debug, Clone)]
pub(crate) enum DeclarationOccurrence<T> {
    Accepted(T),
    Refused(DeclarationRefusalSummary),
}

impl<T> DeclarationOccurrence<T> {
    /// Commit an accepted value into its owner's table and carry the ledger's own
    /// payload out. Refusals pass through untouched, so the commit and the ledger
    /// entry cannot disagree about which declarations were accepted.
    pub(crate) fn map_accepted<U>(self, commit: impl FnOnce(T) -> U) -> DeclarationOccurrence<U> {
        match self {
            Self::Accepted(value) => DeclarationOccurrence::Accepted(commit(value)),
            Self::Refused(summary) => DeclarationOccurrence::Refused(summary),
        }
    }
}

/// What a name resolves to in one namespace.
#[derive(Debug)]
pub(crate) enum Binding<'a, T> {
    Accepted(&'a T),
    /// The key is declared and refused, with the handle that carries the cause
    /// through a `Copy` resolution result and the summary that renders it.
    Refused(DeclarationRefusalId, &'a DeclarationRefusalSummary),
    /// The key was never declared — a genuine absence, and the only case a
    /// not-in-scope report may describe.
    Absent,
}

/// Which occurrence a key resolves to.
#[derive(Debug, Clone, Copy)]
enum Selected {
    Accepted(usize),
    Refused(DeclarationRefusalId),
}

/// What the index holds for one declared key.
#[derive(Debug, Clone, Copy)]
struct KeyOccurrences {
    /// The key's first occurrence — what [`DeclarationLedger::lookup`] answers with.
    /// A refused declaration occupies its name from where it is written, so a later
    /// accepted duplicate does not displace it, exactly as a later accepted
    /// duplicate of an accepted declaration does not.
    first: Selected,
    /// The merged refusal every refused occurrence of this key folds into, which is
    /// `first`'s own when the first occurrence was refused.
    ///
    /// Recorded even when an accepted occurrence answers the lookup: a refusal
    /// standing behind an accepted duplicate is still a refusal the source wrote,
    /// and a completeness predicate that could not see it would call a project
    /// holding a refused declaration whole.
    refused: Option<DeclarationRefusalId>,
}

/// Every occurrence of every declared key in one namespace, in declaration order.
///
/// Layer 1 (`occurrences`) is the sole authority for source order and identity;
/// `index` is lookup-only, appended in lockstep, and never iterated to select a
/// cause or to emit bytes — the discipline `Monomorph` already holds. A divergence
/// between the two is [`DeclarationIndexDrift`], not a silent wrong answer.
pub(crate) struct DeclarationLedger<K, T> {
    namespace: DeclarationNamespace,
    occurrences: Vec<(K, DeclarationOccurrence<T>)>,
    index: BTreeMap<K, KeyOccurrences>,
    /// Positions in `occurrences` of the merged refusal for each refused key,
    /// addressed by [`DeclarationRefusalId`].
    refusals: Vec<usize>,
    budget: DeclarationBudget,
}

/// Layer 1 and the lookup index disagree: a `DeclarationRefusalId` addresses a
/// position that does not hold a refusal, or an index entry names a position
/// outside the occurrence list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclarationIndexDrift;

impl<K, T> DeclarationLedger<K, T> {
    /// An empty ledger for `namespace`, charging its retentions against `budget`.
    /// There is no `Default`: an untagged ledger would mint ids that compare equal
    /// to another namespace's, and an unbudgeted one would retain off the books.
    pub(crate) fn new(namespace: DeclarationNamespace, budget: DeclarationBudget) -> Self {
        Self {
            namespace,
            occurrences: Vec::new(),
            index: BTreeMap::new(),
            refusals: Vec::new(),
            budget,
        }
    }
}

impl<K: Ord + Clone, T> DeclarationLedger<K, T> {
    /// Record one occurrence of `key`.
    ///
    /// The key's *first* occurrence wins the lookup, accepted or refused. A refused
    /// declaration occupies its name from where it is written, so a later duplicate
    /// of a refused key is a name conflict exactly as a duplicate of an accepted one
    /// is, and a later accepted duplicate does not displace the refusal any more than
    /// it would displace an earlier acceptance. Every such shape is separately
    /// reported by its namespace's own duplicate check.
    pub(crate) fn declare(
        &mut self,
        key: K,
        occurrence: DeclarationOccurrence<T>,
    ) -> Result<(), DeclarationLedgerFull> {
        let position = self.occurrences.len();
        let occurrence = match occurrence {
            DeclarationOccurrence::Accepted(value) => {
                self.index.entry(key.clone()).or_insert(KeyOccurrences {
                    first: Selected::Accepted(position),
                    refused: None,
                });
                DeclarationOccurrence::Accepted(value)
            }
            // A further refusal of a key already refused merges into the first: the
            // count rises, no second name is retained, no bytes are charged, and no
            // second occurrence is recorded.
            DeclarationOccurrence::Refused(summary) => {
                match self.index.get(&key).and_then(|entry| entry.refused) {
                    Some(id) => {
                        self.merge_refusal(id, summary);
                        return Ok(());
                    }
                    None => {
                        self.budget.charge(summary.retained_owned_bytes())?;
                        let id = DeclarationRefusalId {
                            namespace: self.namespace,
                            index: self.refusals.len() as u32,
                        };
                        self.refusals.push(position);
                        self.index
                            .entry(key.clone())
                            .and_modify(|entry| entry.refused = Some(id))
                            .or_insert(KeyOccurrences {
                                first: Selected::Refused(id),
                                refused: Some(id),
                            });
                        DeclarationOccurrence::Refused(summary)
                    }
                }
            }
        };
        self.occurrences.push((key, occurrence));
        Ok(())
    }

    /// Fold a further refusal of an already-refused key into the summary `id`
    /// addresses. Drift leaves the merged summary untouched: the first refusal
    /// remains the reported cause, which is what a steer reads.
    fn merge_refusal(&mut self, id: DeclarationRefusalId, summary: DeclarationRefusalSummary) {
        let Some(at) = self.refusals.get(id.index as usize).copied() else {
            return;
        };
        if let Some((_, DeclarationOccurrence::Refused(first))) = self.occurrences.get_mut(at) {
            first.merge(summary);
        }
    }

    /// What `key` resolves to: its first occurrence — the accepted value, or the
    /// merged refusal — else a genuine absence.
    ///
    /// Drift between layer 1 and the index is reported, never answered: `Absent` is
    /// a statement about the source — that nothing declared this key — and a ledger
    /// that answered it for its own incoherence would put a fabricated absence back
    /// at the use site, which is the defect this module exists to remove. An
    /// invariant is not a binding, so it does not become a `Binding` variant.
    pub(crate) fn lookup(&self, key: &K) -> Result<Binding<'_, T>, DeclarationIndexDrift> {
        match self.index.get(key).map(|entry| entry.first) {
            Some(Selected::Accepted(at)) => match self.occurrences.get(at) {
                Some((_, DeclarationOccurrence::Accepted(value))) => Ok(Binding::Accepted(value)),
                _ => Err(DeclarationIndexDrift),
            },
            Some(Selected::Refused(id)) => Ok(Binding::Refused(id, self.refusal(id)?)),
            None => Ok(Binding::Absent),
        }
    }

    /// Whether `key` has any occurrence, accepted or refused — the duplicate check
    /// a namespace runs before declaring, so a refused declaration still occupies
    /// its name.
    pub(crate) fn declared(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    /// The merged refusal `id` addresses.
    ///
    /// An id minted by another namespace's ledger is drift, not a neighbouring
    /// row: the tag is checked before the index is used, so the only way to read a
    /// summary is through the ledger that wrote it.
    pub(crate) fn refusal(
        &self,
        id: DeclarationRefusalId,
    ) -> Result<&DeclarationRefusalSummary, DeclarationIndexDrift> {
        if id.namespace != self.namespace {
            return Err(DeclarationIndexDrift);
        }
        let at = *self
            .refusals
            .get(id.index as usize)
            .ok_or(DeclarationIndexDrift)?;
        match self.occurrences.get(at) {
            Some((_, DeclarationOccurrence::Refused(summary))) => Ok(summary),
            _ => Err(DeclarationIndexDrift),
        }
    }

    /// Every declared key, accepted or refused — the did-you-mean corpus, so a
    /// near-miss on a refused name still suggests it.
    pub(crate) fn keys(&self) -> impl Iterator<Item = &K> {
        self.index.keys()
    }

    /// The refused declarations in source order, one per refused key — the merged
    /// summary each refused key answers with, whether or not an accepted duplicate
    /// of the same name answers the lookup.
    ///
    /// A namespace whose *declared* set is observed independently of its accepted
    /// set reads it from here and `accepted()` together: a refused declaration is
    /// still a declaration the source wrote, and a derivation that walks only the
    /// accepted set silently narrows what it derives.
    pub(crate) fn refused(&self) -> impl Iterator<Item = (&K, &DeclarationRefusalSummary)> {
        self.refusals
            .iter()
            .filter_map(|at| match self.occurrences.get(*at) {
                Some((key, DeclarationOccurrence::Refused(summary))) => Some((key, summary)),
                _ => None,
            })
    }

    /// Every accepted occurrence in source order, including a repeat of a key an
    /// earlier occurrence already answers.
    ///
    /// The function table is the one namespace that needs this: a repeated function
    /// name is reported by its own duplicate check and still lowers a body, so the
    /// image slot count follows the occurrences the source wrote rather than the
    /// names that survived. Every other namespace reads [`Self::accepted`].
    pub(crate) fn accepted_occurrences(&self) -> impl Iterator<Item = (&K, &T)> {
        self.occurrences
            .iter()
            .filter_map(|(key, occurrence)| match occurrence {
                DeclarationOccurrence::Accepted(value) => Some((key, value)),
                DeclarationOccurrence::Refused(_) => None,
            })
    }

    /// The accepted declarations in source order, one per key, and only where the
    /// key's first occurrence is that acceptance: exactly the occurrences
    /// [`Self::lookup`] answers with, so what a namespace builds from this iterator
    /// and what a use site resolves against cannot disagree.
    ///
    /// A namespace whose order is observed — image slot order, field order — reads
    /// its accepted set from here rather than accumulating a parallel vector beside
    /// the ledger, which is what keeps the ledger the single authority for which
    /// declarations survived.
    ///
    /// A namespace whose *occurrences* take slots rather than its keys reads
    /// [`Self::accepted_occurrences`] instead.
    pub(crate) fn accepted(&self) -> impl Iterator<Item = (&K, &T)> {
        self.occurrences
            .iter()
            .enumerate()
            .filter_map(move |(at, (key, occurrence))| match occurrence {
                DeclarationOccurrence::Accepted(value)
                    if matches!(
                        self.index.get(key).map(|entry| entry.first),
                        Some(Selected::Accepted(first)) if first == at
                    ) =>
                {
                    Some((key, value))
                }
                _ => None,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> SourceSpan {
        SourceSpan {
            start_byte: 0,
            end_byte: 1,
            line: 1,
            column: 1,
        }
    }

    fn file() -> FileRef {
        FileRef::admitted(0)
    }

    /// Every summary in these tests is minted through the one production
    /// constructor, so the pushed row and the retained cause stay coupled here too.
    fn refusal(name: &str) -> DeclarationRefusalSummary {
        refused(name, &mut DiagnosticCollector::new())
    }

    fn refused(name: &str, diagnostics: &mut DiagnosticCollector) -> DeclarationRefusalSummary {
        let (identity, _) = FileIdentity::validate("src/main.mw").expect("a valid source path");
        refuse(
            diagnostics,
            DeclarationSite {
                name,
                file: &identity,
                at: file(),
                span: span(),
            },
            "check.type",
            "refused".to_string(),
        )
    }

    fn ledger() -> DeclarationLedger<String, u32> {
        DeclarationLedger::new(DeclarationNamespace::Constant, DeclarationBudget::default())
    }

    #[test]
    fn an_undeclared_key_is_absent() {
        let ledger = ledger();
        assert!(matches!(
            ledger.lookup(&"a".to_string()),
            Ok(Binding::Absent)
        ));
    }

    #[test]
    fn a_refused_key_is_refused_not_absent() {
        let mut ledger = ledger();
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        match ledger.lookup(&"a".to_string()) {
            Ok(Binding::Refused(_, summary)) => {
                assert_eq!(summary.name(), "a");
                assert_eq!(summary.code(), "check.type");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_refused_key_occupies_its_name() {
        let mut ledger = ledger();
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        assert!(ledger.declared(&"a".to_string()));
    }

    #[test]
    fn the_first_accepted_occurrence_wins() {
        let mut ledger = ledger();
        ledger
            .declare("a".to_string(), DeclarationOccurrence::Accepted(1))
            .expect("within budget");
        ledger
            .declare("a".to_string(), DeclarationOccurrence::Accepted(2))
            .expect("within budget");
        assert!(matches!(
            ledger.lookup(&"a".to_string()),
            Ok(Binding::Accepted(1))
        ));
    }

    #[test]
    fn an_accepted_occurrence_outranks_a_later_refusal() {
        let mut ledger = ledger();
        ledger
            .declare("a".to_string(), DeclarationOccurrence::Accepted(1))
            .expect("within budget");
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        assert!(matches!(
            ledger.lookup(&"a".to_string()),
            Ok(Binding::Accepted(1))
        ));
    }

    /// A refusal standing behind an accepted duplicate of its key is still a
    /// refusal the source wrote. The refused set is what a completeness predicate
    /// reads, so a set answering only for the keys a lookup resolves to a refusal
    /// would call a project holding a refused declaration whole.
    #[test]
    fn an_accepted_duplicate_does_not_hide_a_later_refusal() {
        let mut ledger = ledger();
        ledger
            .declare("a".to_string(), DeclarationOccurrence::Accepted(1))
            .expect("within budget");
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        assert_eq!(ledger.refused().count(), 1);
    }

    /// A refused declaration occupies its name from where it is written, so a later
    /// accepted duplicate does not displace it — the same first-occurrence-wins rule
    /// an accepted duplicate meets. Every such shape is separately reported as a
    /// name conflict by the namespace's own duplicate check.
    #[test]
    fn a_refused_occurrence_occupies_its_name_against_a_later_acceptance() {
        let mut ledger = ledger();
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        ledger
            .declare("a".to_string(), DeclarationOccurrence::Accepted(1))
            .expect("within budget");
        match ledger.lookup(&"a".to_string()) {
            Ok(Binding::Refused(_, summary)) => assert_eq!(summary.name(), "a"),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn re_refusing_a_key_merges_and_charges_nothing() {
        let mut ledger = ledger();
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        let after_first = ledger.budget.spent();
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        assert_eq!(ledger.budget.spent(), after_first);
        match ledger.lookup(&"a".to_string()) {
            // The second refusal folds into the first: one retained summary, one
            // reportable cause, and a bounded count of the occurrences behind it.
            Ok(Binding::Refused(_, summary)) => assert_eq!(summary.further, 1),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(ledger.occurrences.len(), 1);
    }

    #[test]
    fn a_refusal_steers_exactly_once() {
        let mut ledger = ledger();
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        let key = "a".to_string();
        let steers = |ledger: &DeclarationLedger<String, u32>| match ledger.lookup(&key) {
            Ok(Binding::Refused(_, summary)) => summary.steer_once(),
            other => panic!("expected a refusal, got {other:?}"),
        };
        assert!(steers(&ledger));
        assert!(!steers(&ledger));
        assert!(!steers(&ledger));
    }

    #[test]
    fn keys_offers_refused_names_as_did_you_mean() {
        let mut ledger = ledger();
        ledger
            .declare("b".to_string(), DeclarationOccurrence::Accepted(1))
            .expect("within budget");
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        let keys: Vec<&str> = ledger.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["a", "b"]);
    }

    /// `accepted()` is what an order-observing namespace builds from, so it must
    /// answer exactly what `lookup` does: source order, refusals skipped, and one
    /// row per key even when a key is declared twice.
    #[test]
    fn accepted_is_source_order_one_row_per_key() {
        let mut ledger = ledger();
        for (key, occurrence) in [
            ("b".to_string(), DeclarationOccurrence::Accepted(1)),
            (
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            ),
            ("c".to_string(), DeclarationOccurrence::Accepted(2)),
            ("b".to_string(), DeclarationOccurrence::Accepted(3)),
        ] {
            ledger.declare(key, occurrence).expect("within budget");
        }
        let accepted: Vec<(&str, u32)> = ledger
            .accepted()
            .map(|(key, value)| (key.as_str(), *value))
            .collect();
        assert_eq!(accepted, vec![("b", 1), ("c", 2)]);
    }

    #[test]
    fn crossing_the_byte_ceiling_is_a_typed_limit_not_a_dropped_key() {
        let mut ledger: DeclarationLedger<String, u32> =
            DeclarationLedger::new(DeclarationNamespace::Constant, DeclarationBudget::default());
        let wide = "n".repeat(4096);
        let mut diagnostics = DiagnosticCollector::new();
        let mut declared = 0usize;
        loop {
            let key = format!("{wide}{declared}");
            let summary = refused(&key, &mut diagnostics);
            match ledger.declare(key, DeclarationOccurrence::Refused(summary)) {
                Ok(()) => declared += 1,
                Err(DeclarationLedgerFull) => break,
            }
            assert!(declared < 4096, "the ceiling must bind before this");
        }
        assert!(ledger.budget.spent() <= MAX_DECLARATION_LEDGER_BYTES);
    }

    #[test]
    fn a_drifted_refusal_id_is_typed_drift_not_a_wrong_summary() {
        let ledger = ledger();
        assert_eq!(
            ledger.refusal(DeclarationRefusalId {
                namespace: DeclarationNamespace::Constant,
                index: 7,
            }),
            Err(DeclarationIndexDrift)
        );
    }

    /// A1 — an id is valid only in the ledger that minted it. Two namespaces mint
    /// index 0, and reading one's id out of the other is drift rather than the
    /// neighbouring summary, which is what keeps `join`'s same-id rule sound.
    #[test]
    fn an_id_from_another_namespace_is_drift_not_a_neighbouring_summary() {
        let mut constants = ledger();
        constants
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        let mut types: DeclarationLedger<String, u32> = DeclarationLedger::new(
            DeclarationNamespace::NamedType,
            DeclarationBudget::default(),
        );
        types
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");

        let Ok(Binding::Refused(from_constants, _)) = constants.lookup(&"a".to_string()) else {
            panic!("expected a refusal");
        };
        let Ok(Binding::Refused(from_types, _)) = types.lookup(&"a".to_string()) else {
            panic!("expected a refusal");
        };
        assert_ne!(from_constants, from_types);
        assert_eq!(types.refusal(from_constants), Err(DeclarationIndexDrift));
        assert_eq!(constants.refusal(from_types), Err(DeclarationIndexDrift));
    }
}
