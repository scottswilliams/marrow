//! The fixed inline eight-slot table-policy ledger and its independent audit.
//!
//! The ledger is an **observer, not an authority**: each slot records the earliest
//! policy crossing of its kind in canonical validation order, the order the legacy
//! policy walk visits its candidates. It stores no counts, authorizes no encode or
//! narrowing, and the still-authoritative legacy walk (`crate::measure`) remains the
//! public candidate owner; every applicable table-owned verdict is shadow-compared
//! against this ledger at the fence, so a ledger that drifts from the draft state it
//! observes is a producer invariant, never a changed verdict.
//!
//! The slot array index is the canonical kind rank — the legacy walk's candidate
//! order: Strings, StringBytes, Consts, Types, Enums, Collections, Roots, Sites.
//! "Earliest" is earliest in that order, never earliest in time: a later-in-time
//! Strings crossing outranks an earlier-in-time Consts crossing.

use crate::bounds;
use crate::draft::ImageDraft;

/// The number of ledger slots: the eight owned table-policy families, fixed.
pub(crate) const TABLE_POLICY_KIND_COUNT: usize = 8;

/// The closed owned policy-kind set, in canonical validation order. The declaration
/// order IS the legacy walk's candidate order; [`TablePolicyKind::rank`] is the slot
/// index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TablePolicyKind {
    Strings,
    StringBytes,
    Consts,
    Types,
    Enums,
    Collections,
    Roots,
    Sites,
}

impl TablePolicyKind {
    /// The canonical kind rank: this kind's slot index and its position in the legacy
    /// walk's candidate order.
    pub(crate) const fn rank(self) -> usize {
        match self {
            TablePolicyKind::Strings => 0,
            TablePolicyKind::StringBytes => 1,
            TablePolicyKind::Consts => 2,
            TablePolicyKind::Types => 3,
            TablePolicyKind::Enums => 4,
            TablePolicyKind::Collections => 5,
            TablePolicyKind::Roots => 6,
            TablePolicyKind::Sites => 7,
        }
    }
}

/// The canonical legacy-validation coordinate of one kind's earliest crossing: the
/// wide logical row ordinal of the first row the kind's policy predicate refuses
/// under the legacy walk's own iteration.
///
/// For a count kind (Strings, Consts, Types, Enums, Collections, Roots) this is
/// exactly the ordinal `MAX_<kind>` — the N+1 row; for StringBytes it is the row
/// ordinal of the first over-`MAX_STRING_BYTES` string in pool order. It is never a
/// mutation stamp: every slot value is recomputable from final draft state alone,
/// which is what keeps the audit independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CurrentValidationOccurrence {
    row: u32,
}

impl CurrentValidationOccurrence {
    pub(crate) const fn at_row(row: u32) -> Self {
        Self { row }
    }
}

/// A slot index into the fixed eight-slot array: the canonical kind rank of the
/// cached subset minimum. Caching the index preserves both the kind rank (the index
/// itself) and the row coordinate (read through the slot on demand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LedgerSlotIndex(usize);

impl LedgerSlotIndex {
    pub(crate) const fn of(kind: TablePolicyKind) -> Self {
        Self(kind.rank())
    }
}

/// The fixed inline eight-slot policy ledger: one optional canonical occurrence per
/// owned kind plus the cached lowest-ranked occupied slot. Allocation-free, `Copy`
/// (the one armed-inverse copy at transaction admission), no vector, map, payload
/// spelling, span, cause, availability witness, or image byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TablePolicyLedger {
    slots: [Option<CurrentValidationOccurrence>; TABLE_POLICY_KIND_COUNT],
    cached_minimum: Option<LedgerSlotIndex>,
}

impl TablePolicyLedger {
    /// The vacant ledger a fresh draft holds.
    pub(crate) const fn vacant() -> Self {
        Self {
            slots: [None; TABLE_POLICY_KIND_COUNT],
            cached_minimum: None,
        }
    }

    /// Record one crossing observation. The earliest observation of a kind wins — an
    /// equal-or-later observation in canonical order is idempotent — and the cached
    /// subset minimum is maintained in O(1) on every insert: a newly occupied slot
    /// with a lower rank replaces the cache.
    pub(crate) fn observe(
        &mut self,
        kind: TablePolicyKind,
        occurrence: CurrentValidationOccurrence,
    ) {
        let rank = kind.rank();
        if let Some(slot) = self.slots.get_mut(rank)
            && slot.is_none()
        {
            *slot = Some(occurrence);
            match self.cached_minimum {
                Some(LedgerSlotIndex(current)) if current <= rank => {}
                _ => self.cached_minimum = Some(LedgerSlotIndex(rank)),
            }
        }
    }

    /// The lowest-ranked occupied slot, if any — the kind the legacy walk's first
    /// table-owned candidate must name.
    pub(crate) fn cached_minimum(&self) -> Option<LedgerSlotIndex> {
        self.cached_minimum
    }

    fn slot(&self, kind: TablePolicyKind) -> Option<CurrentValidationOccurrence> {
        self.slots.get(kind.rank()).copied().flatten()
    }
}

/// The independent read-only ledger audit: it recomputes exactly the owned subset
/// from final draft state — per kind, whether the current state is over policy and
/// the canonical coordinate of the crossing — and cross-validates each slot's
/// presence, absence, and value plus the cached subset minimum, byte-exactly.
///
/// It is neither a full candidate decision nor a clean witness: it authorizes no
/// encode, narrowing, or mutation-clean conclusion, and a mismatch (corrupt,
/// missing, extra, stale, wrong-occurrence, wrong-minimum) is a producer invariant.
pub(crate) struct TablePolicyAudit;

impl TablePolicyAudit {
    /// Recompute the owned subset over `draft`'s final state and cross-validate the
    /// ledger, returning the first mismatch's description.
    pub(crate) fn cross_validate(draft: &ImageDraft) -> Result<(), &'static str> {
        Self::cross_validate_against(draft.policy_ledger(), draft)
    }

    /// The comparison core behind [`Self::cross_validate`]: `ledger` is the observed
    /// state under audit, always the draft's own in production.
    fn cross_validate_against(
        ledger: &TablePolicyLedger,
        draft: &ImageDraft,
    ) -> Result<(), &'static str> {
        let mut minimum = None;
        for (kind, expected) in [
            (
                TablePolicyKind::Strings,
                count_crossing(draft.strings().len(), bounds::MAX_STRINGS),
            ),
            (TablePolicyKind::StringBytes, first_over_long(draft)),
            (
                TablePolicyKind::Consts,
                count_crossing(draft.consts().len(), bounds::MAX_CONSTS),
            ),
            (
                TablePolicyKind::Types,
                count_crossing(draft.types().len(), bounds::MAX_TYPES),
            ),
            (
                TablePolicyKind::Enums,
                count_crossing(draft.enums().len(), bounds::MAX_ENUMS),
            ),
            (
                TablePolicyKind::Collections,
                count_crossing(draft.collections().len(), bounds::MAX_COLLECTIONS),
            ),
            (
                TablePolicyKind::Roots,
                count_crossing(draft.root_occurrences().len(), bounds::MAX_ROOTS),
            ),
            (TablePolicyKind::Sites, site_crossing(draft)),
        ] {
            if ledger.slot(kind) != expected {
                return Err("a ledger slot disagrees with the recomputed final draft state");
            }
            if expected.is_some() && minimum.is_none() {
                minimum = Some(LedgerSlotIndex::of(kind));
            }
        }
        if ledger.cached_minimum() != minimum {
            return Err("the cached subset minimum disagrees with the recomputed slots");
        }
        Ok(())
    }
}

/// The canonical occurrence of a count kind's crossing: the N+1 ordinal `MAX_<kind>`,
/// present exactly when the table's row count exceeds its policy maximum.
fn count_crossing(len: usize, max: usize) -> Option<CurrentValidationOccurrence> {
    (len > max).then(|| CurrentValidationOccurrence::at_row(max as u32))
}

/// The canonical Sites occurrence: the virtual zero-based N+1 ordinal `MAX_SITES` —
/// never a physical site-row index or wire id — present exactly when the site plan
/// holds its earliest policy receipt. The receipt's `RowStamp` identity itself is the
/// separate exact cross-check every over-policy ref's provenance is validated against.
fn site_crossing(draft: &ImageDraft) -> Option<CurrentValidationOccurrence> {
    draft
        .site_receipt()
        .map(|_| CurrentValidationOccurrence::at_row(bounds::MAX_SITES as u32))
}

/// The canonical StringBytes occurrence: the pool ordinal of the first
/// over-`MAX_STRING_BYTES` string in pool order.
fn first_over_long(draft: &ImageDraft) -> Option<CurrentValidationOccurrence> {
    draft
        .strings()
        .iter()
        .position(|text| text.len() > bounds::MAX_STRING_BYTES)
        .map(|row| CurrentValidationOccurrence::at_row(row as u32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draft::{DraftTxn, RecordTypeDef, RootOccurrenceDef};
    use crate::product::{DeclarationMemberDef, DeclarationMemberShape};
    use crate::ty::Scalar;
    use crate::{AdmittedGraphInputPlan, ImageDraft, LedgerIdBytes, SemanticTarget};

    /// The image's own admitted-intake ceilings, as one plan.
    fn admitted_plan() -> AdmittedGraphInputPlan {
        AdmittedGraphInputPlan::admit(
            bounds::MAX_ADMITTED_PRODUCT_DECLARATIONS,
            bounds::MAX_ADMITTED_ROOT_OCCURRENCES,
            bounds::MAX_ADMITTED_DECLARATION_COMMANDS,
        )
        .expect("the image's own ceilings are admitted counts")
    }

    /// A distinct 16-byte ledger id seeded by `n`, so every field member below is its own
    /// declaration node and every demand over it a distinct `(occurrence, node, target)`.
    fn field_id(n: usize) -> LedgerIdBytes {
        let mut bytes = [0x50u8; 16];
        bytes[0] = (n & 0xff) as u8;
        bytes[1] = ((n >> 8) & 0xff) as u8;
        LedgerIdBytes::from_bytes(bytes)
    }

    /// Declare one Product wide enough that demanding every field leaf under two roots
    /// crosses `MAX_SITES`, and append one root over it.
    fn declare_wide(draft: &mut DraftTxn<'_>) {
        let name = draft.intern_string("R");
        let record = draft.add_record_type(RecordTypeDef {
            name,
            fields: Vec::new(),
        });
        draft.set_application_identity(LedgerIdBytes::from_bytes([0x0a; 16]));
        let value = draft.value_scalar(Scalar::Int);
        draft
            .declare_product(
                &admitted_plan(),
                LedgerIdBytes::from_bytes([0x0d; 16]),
                record,
                (0..bounds::MAX_SITES)
                    .map(|n| DeclarationMemberDef {
                        parent: None,
                        shape: DeclarationMemberShape::Field {
                            id: field_id(n),
                            required: true,
                            value,
                        },
                    })
                    .collect(),
            )
            .expect("a well-formed declaration");
    }

    /// Demand every field leaf of the declared Product under one fresh root, seeded by
    /// `seed`.
    fn demand_every_leaf_under_a_fresh_root(draft: &mut DraftTxn<'_>, seed: u8, leaves: usize) {
        let name = draft.intern_string(&format!("r{seed}"));
        let root = draft
            .add_root_occurrence(
                &admitted_plan(),
                LedgerIdBytes::from_bytes([0x0d; 16]),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: LedgerIdBytes::from_bytes([seed; 16]),
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        let members = draft
            .product_members(LedgerIdBytes::from_bytes([0x0d; 16]))
            .expect("declared");
        for member in members.iter().take(leaves) {
            let handle = draft
                .bind_occurrence_site(root.occurrence(), member.path(), SemanticTarget::FieldLeaf)
                .expect("a canonical path of this occurrence");
            let _ = draft.request_site(&handle).expect("the binding is live");
        }
    }

    /// A draft whose constant pool has crossed `MAX_CONSTS` through the production
    /// transaction surface, so its true ledger holds exactly the Consts slot.
    fn consts_crossed_draft() -> ImageDraft {
        let mut owner = ImageDraft::new();
        let mut txn = owner
            .begin_transaction(owner.savepoint())
            .expect("a fresh savepoint admits");
        for value in 0..=(bounds::MAX_CONSTS as i64) {
            txn.intern_int(value);
        }
        txn.commit();
        owner
    }

    /// A wrong-occurrence ledger — the right slot occupied at the wrong canonical
    /// coordinate — and a wrong-minimum cache — the right slots under a cache naming a
    /// different kind — are each invariant at the audit, alongside a corrupt slot value
    /// on a kind the draft never crossed.
    #[test]
    fn wrong_occurrence_and_wrong_minimum_ledger_states_are_invariant_at_the_audit() {
        let draft = consts_crossed_draft();
        assert!(
            TablePolicyAudit::cross_validate(&draft).is_ok(),
            "the true ledger audits clean",
        );

        // Wrong occurrence: Consts occupied, but at a coordinate the legacy walk would
        // never report.
        let mut wrong_occurrence = TablePolicyLedger::vacant();
        wrong_occurrence.observe(
            TablePolicyKind::Consts,
            CurrentValidationOccurrence::at_row(bounds::MAX_CONSTS as u32 + 7),
        );
        assert!(
            TablePolicyAudit::cross_validate_against(&wrong_occurrence, &draft).is_err(),
            "a wrong-occurrence slot is invariant",
        );

        // Wrong minimum: the correct slot set under a cache naming a different kind.
        let wrong_minimum = TablePolicyLedger {
            slots: {
                let mut slots = [None; TABLE_POLICY_KIND_COUNT];
                slots[TablePolicyKind::Consts.rank()] = Some(CurrentValidationOccurrence::at_row(
                    bounds::MAX_CONSTS as u32,
                ));
                slots
            },
            cached_minimum: Some(LedgerSlotIndex::of(TablePolicyKind::Sites)),
        };
        assert!(
            TablePolicyAudit::cross_validate_against(&wrong_minimum, &draft).is_err(),
            "a wrong cached minimum is invariant",
        );

        // Corrupt: a slot occupied for a kind the draft never crossed.
        let mut corrupt = TablePolicyLedger::vacant();
        corrupt.observe(
            TablePolicyKind::Consts,
            CurrentValidationOccurrence::at_row(bounds::MAX_CONSTS as u32),
        );
        corrupt.observe(
            TablePolicyKind::Sites,
            CurrentValidationOccurrence::at_row(bounds::MAX_SITES as u32),
        );
        assert!(
            TablePolicyAudit::cross_validate_against(&corrupt, &draft).is_err(),
            "an extra occupied slot is invariant",
        );

        // Missing: the vacant ledger against a genuinely crossed draft.
        assert!(
            TablePolicyAudit::cross_validate_against(&TablePolicyLedger::vacant(), &draft).is_err(),
            "a missing slot is invariant",
        );
    }

    /// The one mint path's crossing populates the Sites slot with the virtual zero-based
    /// N+1 ordinal `MAX_SITES` — never a physical site-row index or wire id — commit
    /// retains it with the cached minimum, no excess physical row is committed, and a
    /// rolled-back crossing restores the vacant slot byte for byte.
    #[test]
    fn a_sites_crossing_populates_and_rolls_back_its_ledger_slot() {
        let mut owner = ImageDraft::new();
        let mut draft = owner
            .begin_transaction(owner.savepoint())
            .expect("a fresh savepoint admits");
        declare_wide(&mut draft);
        demand_every_leaf_under_a_fresh_root(&mut draft, 0x21, bounds::MAX_SITES);
        draft.commit();
        assert_eq!(
            owner.policy_ledger().slot(TablePolicyKind::Sites),
            None,
            "a demand of exactly MAX_SITES is not a crossing",
        );

        // A rolled-back crossing restores the vacant slot with the receipt.
        {
            let mut txn = owner
                .begin_transaction(owner.savepoint())
                .expect("a fresh savepoint admits");
            demand_every_leaf_under_a_fresh_root(&mut txn, 0x22, 1);
            assert_eq!(
                txn.policy_ledger().slot(TablePolicyKind::Sites),
                Some(CurrentValidationOccurrence::at_row(
                    bounds::MAX_SITES as u32
                )),
                "the crossing is observed at the virtual N+1 ordinal",
            );
        }
        assert_eq!(
            owner.policy_ledger().slot(TablePolicyKind::Sites),
            None,
            "the rolled-back crossing restored the vacant slot",
        );

        // A committed crossing retains the slot, the cached minimum, and no excess row.
        let mut txn = owner
            .begin_transaction(owner.savepoint())
            .expect("a fresh savepoint admits");
        demand_every_leaf_under_a_fresh_root(&mut txn, 0x23, 1);
        txn.commit();
        assert_eq!(
            owner.policy_ledger().slot(TablePolicyKind::Sites),
            Some(CurrentValidationOccurrence::at_row(
                bounds::MAX_SITES as u32
            )),
        );
        assert_eq!(
            owner.policy_ledger().cached_minimum(),
            Some(LedgerSlotIndex::of(TablePolicyKind::Sites)),
            "Sites is the only crossed kind, so it is the canonical minimum",
        );
        assert_eq!(
            owner.site_row_count(),
            bounds::MAX_SITES,
            "the crossing committed no excess physical site row",
        );
        assert!(
            TablePolicyAudit::cross_validate(&owner).is_ok(),
            "the audit recomputes the Sites slot from final draft state",
        );
    }

    /// The frozen representation constants: eight slots, and the exact inline
    /// size/alignment of the `Copy` ledger the armed inverse copies at admission.
    #[test]
    fn the_ledger_representation_holds_its_frozen_widths() {
        assert_eq!(
            TABLE_POLICY_KIND_COUNT, 8,
            "the slot count is frozen at eight"
        );
        assert_eq!(
            size_of::<TablePolicyLedger>(),
            80,
            "B_TABLE_POLICY_LEDGER: eight 8-byte optional occurrences plus the cached minimum",
        );
        assert_eq!(align_of::<TablePolicyLedger>(), 8, "the ledger's alignment");
    }

    /// The earliest observation per kind wins; an equal-or-later observation in
    /// canonical order is idempotent.
    #[test]
    fn a_later_observation_of_an_occupied_slot_is_idempotent() {
        let mut ledger = TablePolicyLedger::vacant();
        ledger.observe(
            TablePolicyKind::Consts,
            CurrentValidationOccurrence::at_row(64),
        );
        ledger.observe(
            TablePolicyKind::Consts,
            CurrentValidationOccurrence::at_row(65),
        );
        assert_eq!(
            ledger.slot(TablePolicyKind::Consts),
            Some(CurrentValidationOccurrence::at_row(64)),
        );
    }

    /// A later-in-time crossing of a lower-ranked kind takes the cached minimum: the
    /// ranking is canonical validation order, never mutation-time order.
    #[test]
    fn a_later_lower_ranked_crossing_takes_the_cached_minimum() {
        let mut ledger = TablePolicyLedger::vacant();
        ledger.observe(
            TablePolicyKind::Consts,
            CurrentValidationOccurrence::at_row(64),
        );
        assert_eq!(
            ledger.cached_minimum(),
            Some(LedgerSlotIndex::of(TablePolicyKind::Consts)),
        );
        ledger.observe(
            TablePolicyKind::Strings,
            CurrentValidationOccurrence::at_row(8192),
        );
        assert_eq!(
            ledger.cached_minimum(),
            Some(LedgerSlotIndex::of(TablePolicyKind::Strings)),
        );
        // A higher-ranked crossing afterwards leaves the minimum where it is.
        ledger.observe(
            TablePolicyKind::Roots,
            CurrentValidationOccurrence::at_row(4096),
        );
        assert_eq!(
            ledger.cached_minimum(),
            Some(LedgerSlotIndex::of(TablePolicyKind::Strings)),
        );
    }
}
