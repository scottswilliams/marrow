//! Declared-name ledgers: the one way a declared key leaves the accepted set.
//!
//! A namespace that refuses a declaration must not forget it. Dropping the key
//! makes every later lookup read as *never declared*, so the compiler reports a
//! fabricated absence at each use instead of the cause it already diagnosed at the
//! declaration. A ledger keeps the refused key with a summary of its first refusal,
//! so a lookup answers `Refused` — carrying the declaring code, file, and span —
//! rather than `Absent`.
//!
//! The retained summary is the only owned retention this module adds, and it is
//! charged against [`MAX_DECLARATION_LEDGER_BYTES`] at `declare`. The charge is
//! per refused *key*: re-refusing a key already in the ledger increments a bounded
//! occurrence count and costs nothing, which is what holds amplification to the
//! number of refused declarations rather than the number of uses.

use std::cell::Cell;
use std::collections::BTreeMap;

use marrow_project::FileIdentity;
use marrow_syntax::SourceSpan;

use crate::analysis::FileRef;
use crate::diag::{DiagnosticCollector, IdentityGap, MAX_DIAGNOSTIC_BYTES, SourceDiagnostic};

/// The most owned bytes every declaration ledger in one pass may retain together.
///
/// Declared, not derived from a length: the ledger is live concurrently with the
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

/// A `Copy` handle to one refused declaration, valid only in the ledger that
/// minted it. Refusal causes travel through `ResolveRefusal` as this id so that
/// no owned bytes enter the monomorphization cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DeclarationRefusalId(u32);

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
    file: FileRef,
    span: SourceSpan,
    further: u16,
    gap: Option<IdentityGap>,
    /// Whether a use site has already been steered to this cause. A `Cell` because
    /// the flag is the report-once record and every namespace ledger is read through
    /// a shared reference during lowering; the alternative is the parallel
    /// `&mut BTreeSet<String>` of steered names this ledger replaces.
    steered: Cell<bool>,
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
    at: Declared<'_>,
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
    at: Declared<'_>,
    row: SourceDiagnostic,
) -> DeclarationRefusalSummary {
    let summary = DeclarationRefusalSummary {
        name: at.name.to_string(),
        code: row.code(),
        file: at.at,
        span: at.span,
        further: 0,
        gap: None,
        steered: Cell::new(false),
    };
    diagnostics.push(row);
    summary
}

/// Where one declaration is written: its name, its file in both the owned spelling
/// a diagnostic renders and the `Copy` coordinate a summary retains, and its span.
#[derive(Clone, Copy)]
pub(crate) struct Declared<'a> {
    pub(crate) name: &'a str,
    pub(crate) file: &'a FileIdentity,
    pub(crate) at: FileRef,
    pub(crate) span: SourceSpan,
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
            file: _,
            span: _,
            further,
            gap,
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

/// What a name resolves to in one namespace.
#[derive(Debug)]
pub(crate) enum Binding<'a, T> {
    Accepted(&'a T),
    Refused(&'a DeclarationRefusalSummary),
    /// The key was never declared — a genuine absence, and the only case a
    /// not-in-scope report may describe.
    Absent,
}

/// Which occurrence a key resolves to, and where the merged refusal lives.
#[derive(Debug, Clone, Copy)]
enum Selected {
    Accepted(usize),
    Refused(DeclarationRefusalId),
}

/// Every occurrence of every declared key in one namespace, in declaration order.
///
/// Layer 1 (`occurrences`) is the sole authority for source order and identity;
/// `index` is lookup-only, appended in lockstep, and never iterated to select a
/// cause or to emit bytes — the discipline `Monomorph` already holds. A divergence
/// between the two is [`DeclarationIndexDrift`], not a silent wrong answer.
pub(crate) struct DeclarationLedger<K, T> {
    occurrences: Vec<(K, DeclarationOccurrence<T>)>,
    index: BTreeMap<K, Selected>,
    /// Positions in `occurrences` of the merged refusal for each refused key,
    /// addressed by [`DeclarationRefusalId`].
    refusals: Vec<usize>,
    owned_bytes: usize,
}

/// Layer 1 and the lookup index disagree: a `DeclarationRefusalId` addresses a
/// position that does not hold a refusal, or an index entry names a position
/// outside the occurrence list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclarationIndexDrift;

impl<K, T> Default for DeclarationLedger<K, T> {
    fn default() -> Self {
        Self {
            occurrences: Vec::new(),
            index: BTreeMap::new(),
            refusals: Vec::new(),
            owned_bytes: 0,
        }
    }
}

impl<K: Ord + Clone, T> DeclarationLedger<K, T> {
    /// Record one occurrence of `key`.
    ///
    /// The first accepted occurrence wins the lookup, preserving
    /// first-declaration-wins; a refused occurrence still occupies the name, so a
    /// later duplicate of a refused key is a name conflict exactly as a duplicate
    /// of an accepted one is.
    pub(crate) fn declare(
        &mut self,
        key: K,
        occurrence: DeclarationOccurrence<T>,
    ) -> Result<(), DeclarationLedgerFull> {
        let position = self.occurrences.len();
        match occurrence {
            DeclarationOccurrence::Accepted(value) => {
                self.index
                    .entry(key.clone())
                    .or_insert(Selected::Accepted(position));
                self.occurrences
                    .push((key, DeclarationOccurrence::Accepted(value)));
            }
            DeclarationOccurrence::Refused(summary) => {
                match self.index.get(&key).copied() {
                    // An accepted occurrence already answers this key; the refusal
                    // is retained in source order but costs no lookup change.
                    Some(Selected::Accepted(_)) => {
                        self.charge(summary.retained_owned_bytes())?;
                        self.occurrences
                            .push((key, DeclarationOccurrence::Refused(summary)));
                    }
                    // A second refusal of one key merges into the first: the count
                    // rises, no second name is retained, and no bytes are charged.
                    Some(Selected::Refused(id)) => {
                        let at = self.refusals[id.0 as usize];
                        if let Some((_, DeclarationOccurrence::Refused(first))) =
                            self.occurrences.get_mut(at)
                        {
                            first.merge(summary);
                        }
                    }
                    None => {
                        self.charge(summary.retained_owned_bytes())?;
                        let id = DeclarationRefusalId(self.refusals.len() as u32);
                        self.refusals.push(position);
                        self.index.insert(key.clone(), Selected::Refused(id));
                        self.occurrences
                            .push((key, DeclarationOccurrence::Refused(summary)));
                    }
                }
            }
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), DeclarationLedgerFull> {
        let charged = self.owned_bytes.saturating_add(bytes);
        if charged > MAX_DECLARATION_LEDGER_BYTES {
            return Err(DeclarationLedgerFull);
        }
        self.owned_bytes = charged;
        Ok(())
    }

    /// What `key` resolves to: its first accepted occurrence, else the merged
    /// refusal for the key, else a genuine absence.
    pub(crate) fn lookup(&self, key: &K) -> Binding<'_, T> {
        match self.index.get(key) {
            Some(Selected::Accepted(at)) => match self.occurrences.get(*at) {
                Some((_, DeclarationOccurrence::Accepted(value))) => Binding::Accepted(value),
                _ => Binding::Absent,
            },
            Some(Selected::Refused(id)) => match self.refusal(*id) {
                Ok(summary) => Binding::Refused(summary),
                Err(DeclarationIndexDrift) => Binding::Absent,
            },
            None => Binding::Absent,
        }
    }

    /// Whether `key` has any occurrence, accepted or refused — the duplicate check
    /// a namespace runs before declaring, so a refused declaration still occupies
    /// its name.
    pub(crate) fn declared(&self, key: &K) -> bool {
        self.index.contains_key(key)
    }

    /// The merged refusal `id` addresses.
    pub(crate) fn refusal(
        &self,
        id: DeclarationRefusalId,
    ) -> Result<&DeclarationRefusalSummary, DeclarationIndexDrift> {
        let at = *self
            .refusals
            .get(id.0 as usize)
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
            Declared {
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
        DeclarationLedger::default()
    }

    #[test]
    fn an_undeclared_key_is_absent() {
        let ledger = ledger();
        assert!(matches!(ledger.lookup(&"a".to_string()), Binding::Absent));
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
            Binding::Refused(summary) => {
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
            Binding::Accepted(1)
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
            Binding::Accepted(1)
        ));
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
        let after_first = ledger.owned_bytes;
        ledger
            .declare(
                "a".to_string(),
                DeclarationOccurrence::Refused(refusal("a")),
            )
            .expect("within budget");
        assert_eq!(ledger.owned_bytes, after_first);
        match ledger.lookup(&"a".to_string()) {
            // The second refusal folds into the first: one retained summary, one
            // reportable cause, and a bounded count of the occurrences behind it.
            Binding::Refused(summary) => assert_eq!(summary.further, 1),
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
            Binding::Refused(summary) => summary.steer_once(),
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

    #[test]
    fn crossing_the_byte_ceiling_is_a_typed_limit_not_a_dropped_key() {
        let mut ledger: DeclarationLedger<String, u32> = DeclarationLedger::default();
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
        assert!(ledger.owned_bytes <= MAX_DECLARATION_LEDGER_BYTES);
    }

    #[test]
    fn a_drifted_refusal_id_is_typed_drift_not_a_wrong_summary() {
        let ledger = ledger();
        assert_eq!(
            ledger.refusal(DeclarationRefusalId(7)),
            Err(DeclarationIndexDrift)
        );
    }
}
