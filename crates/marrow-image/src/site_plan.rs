//! The bounded operation-site demand plan: the one owner of the site table's rows, its
//! deduplicating demand map, and its capacity policy.
//!
//! Every operation site an image carries is requested here, and the plan checks vacant
//! capacity *before* it mints a numeric id. A fitting [`SiteId`] is therefore always
//! inside `0..MAX_SITES`, and no arithmetic on the table's length ever produces one.
//!
//! Before this owner existed the site table was appended to directly and its id was the
//! table's length narrowed to `u16`, with the bound seen only at `encode()`. A producer
//! could request past `u16::MAX` distinct durable nodes, receive a wrapped id, and hand
//! two distinct nodes the same site operand.
//!
//! Crossing the cap is **nonblocking**: the plan saturates its logical demand count at
//! `MAX_SITES + 1`, records the earliest crossing once, and keeps answering. A demand it
//! already retains still resolves to the id it was given, so a repeated reference never
//! begins to fail; every other post-cap demand is refused an id rather than aliasing a
//! fitting one. The encoder projects the saturated logical count through the Sites bound,
//! so an image whose demand crossed the cap is refused there as it always was.

use std::collections::HashMap;

use crate::bounds::MAX_SITES;
use crate::draft::{SiteDef, SiteId};
use crate::semantic::{SemanticPath, SemanticTarget};

/// The one demand a site row answers: the addressed node's whole semantic path and the
/// operation target over it.
///
/// The key is the whole path, never the terminal step's ledger id. A Product member
/// declaration below two root occurrences has one declaration id and two distinct paths,
/// so a terminal-id key would hand one occurrence the other occurrence's site.
type SiteDemandKey = (SemanticPath, SemanticTarget);

/// The record that a plan's demand crossed `MAX_SITES`. The plan retains no row or key
/// past the crossing, so this is what remains of every demand beyond it.
///
/// It is a typed fact rather than a flag: "this plan's demand exceeded its capacity" is a
/// state the encoder reads and the draft cannot un-observe, and giving it a name keeps a
/// bare boolean from standing in for it. One receipt is recorded, at the earliest
/// crossing; later refusals do not replace it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SitePolicyReceipt;

/// The bounded site demand plan.
#[derive(Debug, Default, Clone)]
pub(crate) struct SiteDemandPlan {
    /// The retained site rows, in emission order. A row's index is its [`SiteId`].
    rows: Vec<SiteDef>,
    /// The retained demands, so a repeated reference to one `(node, target)` returns the
    /// id already minted for it. The rows vector stays the sole table authority and
    /// emission order; this only makes the table carry a site per *demanded* node rather
    /// than one per declared graph node.
    retained: HashMap<SiteDemandKey, SiteId>,
    /// The earliest crossing, recorded once. With the rows it is the whole logical demand
    /// count: a plan retains exactly one row per fitting demand, and the receipt stands for
    /// every demand past the cap, so no separate counter can drift from the table.
    receipt: Option<SitePolicyReceipt>,
}

impl SiteDemandPlan {
    /// Mint-or-return the site answering `def`.
    ///
    /// `None` is the over-policy answer: the plan has no vacant capacity and does not
    /// already retain this demand, so there is no id it can give that would not alias a
    /// fitting one.
    pub(crate) fn request(&mut self, def: SiteDef) -> Option<SiteId> {
        let key = (def.path.clone(), def.target);
        if let Some(existing) = self.retained.get(&key) {
            return Some(*existing);
        }
        // Vacant capacity is checked before any numeric id is minted, so the fitting range
        // stays exactly `0..MAX_SITES` and the conversion below cannot narrow a length
        // into an id that aliases one.
        if self.rows.len() >= MAX_SITES {
            return self.saturate();
        }
        let Ok(ordinal) = u16::try_from(self.rows.len()).map(SiteId::from_ordinal) else {
            return self.saturate();
        };
        self.rows.push(def);
        self.retained.insert(key, ordinal);
        Some(ordinal)
    }

    /// Record the crossing and refuse an id. The logical count saturates at
    /// `MAX_SITES + 1` — one past the cap is all the encoder's bound needs to read, and
    /// counting every excess demand would retain unbounded work for a refused image.
    fn saturate(&mut self) -> Option<SiteId> {
        self.receipt.get_or_insert(SitePolicyReceipt);
        None
    }

    /// The retained site rows, in emission order.
    pub(crate) fn rows(&self) -> &[SiteDef] {
        &self.rows
    }

    /// The logical demand count, saturating at `MAX_SITES + 1`. The encoder's Sites bound
    /// reads this rather than the row count: the plan refuses to mint past its capacity, so
    /// the row count can never exceed the bound and reading it would disable the check.
    pub(crate) fn demanded(&self) -> usize {
        self.rows.len() + usize::from(self.receipt.is_some())
    }

    /// Discard every row appended after `rows`, dropping exactly those rows' demand keys.
    ///
    /// The purge is reconstructed from the removed rows, so it is proportional to the
    /// discarded suffix rather than to the whole table. A plan whose demand crossed the
    /// cap retains no excess row or key, so its saturated count and receipt are not
    /// restored by discarding a suffix: the crossing happened.
    pub(crate) fn truncate(&mut self, rows: usize) {
        for row in self.rows.drain(rows..) {
            self.retained.remove(&(row.path, row.target));
        }
        // A saturated plan keeps its receipt: the crossing is not undone by discarding a
        // suffix, and the demands it refused were never retained to restore.
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SITES, SiteDemandPlan};
    use crate::draft::SiteDef;
    use crate::durable_id::LedgerIdBytes;
    use crate::semantic::{SemanticPath, SemanticStep, SemanticStepKind};

    /// A distinct field-leaf demand below one root, seeded by `n`.
    fn leaf(n: usize) -> SiteDef {
        let mut bytes = [0x50u8; 16];
        bytes[0] = u8::try_from(n & 0xff).expect("masked to one byte");
        bytes[1] = u8::try_from((n >> 8) & 0xff).expect("masked to one byte");
        bytes[2] = u8::try_from((n >> 16) & 0xff).expect("masked to one byte");
        SiteDef::field_leaf(
            SemanticPath::root(
                LedgerIdBytes::from_bytes([0x0a; 16]),
                LedgerIdBytes::from_bytes([0x0b; 16]),
            )
            .child(SemanticStep::new(
                SemanticStepKind::Field,
                LedgerIdBytes::from_bytes(bytes),
            )),
        )
    }

    /// The plan mints exactly its capacity and never narrows a table length into an id.
    /// Requesting `u16::MAX + 2` distinct demands is the shape that handed the 65,536th
    /// durable node the id of the first one.
    #[test]
    fn no_demand_past_the_cap_ever_aliases_a_fitting_site() {
        let mut plan = SiteDemandPlan::default();
        let mut minted = Vec::new();
        for n in 0..(usize::from(u16::MAX) + 2) {
            if let Some(id) = plan.request(leaf(n)) {
                minted.push(id.index());
            }
        }

        assert_eq!(
            minted.len(),
            MAX_SITES,
            "the plan mints exactly its capacity"
        );
        assert!(
            minted.iter().all(|id| usize::from(*id) < MAX_SITES),
            "every minted id is inside the table's capacity",
        );
        let mut unique = minted.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            minted.len(),
            "no two distinct durable demands share one site id",
        );
    }

    /// The logical count saturates one past the cap and the earliest crossing is recorded
    /// once, so the encoder's Sites bound still fires for an image whose demand crossed.
    #[test]
    fn crossing_the_cap_saturates_the_count_and_records_one_earliest_receipt() {
        let mut plan = SiteDemandPlan::default();
        for n in 0..MAX_SITES {
            plan.request(leaf(n))
                .expect("every demand below the cap fits");
        }
        assert_eq!(
            plan.demanded(),
            MAX_SITES,
            "no receipt is recorded at exactly MAX_SITES, so the demand still fits",
        );

        assert_eq!(
            plan.request(leaf(MAX_SITES)),
            None,
            "the first excess is refused"
        );
        assert_eq!(
            plan.demanded(),
            MAX_SITES + 1,
            "the crossing is recorded and the logical count saturates one past the cap",
        );

        for n in (MAX_SITES + 1)..(MAX_SITES + 64) {
            assert_eq!(plan.request(leaf(n)), None);
        }
        assert_eq!(
            plan.demanded(),
            MAX_SITES + 1,
            "the logical count saturates rather than counting every refused demand",
        );
        assert_eq!(plan.rows().len(), MAX_SITES, "no excess row is retained");
    }

    /// The crossing is nonblocking: a demand the plan already retains keeps resolving to
    /// the id it was given.
    #[test]
    fn a_retained_demand_resolves_after_the_crossing() {
        let mut plan = SiteDemandPlan::default();
        let first = plan.request(leaf(0)).expect("the first demand fits");
        for n in 1..=MAX_SITES {
            let _ = plan.request(leaf(n));
        }
        assert_eq!(
            plan.request(leaf(0)),
            Some(first),
            "a repeated reference returns the id already minted for it",
        );
    }

    /// Discarding a suffix drops exactly that suffix's demands, and a demand that survived
    /// keeps its id — the purge cannot remove a surviving row's key.
    #[test]
    fn discarding_a_suffix_keeps_every_surviving_demand_resolvable() {
        let mut plan = SiteDemandPlan::default();
        let kept = plan.request(leaf(0)).expect("fits");
        let mark = plan.rows().len();
        plan.request(leaf(1)).expect("fits");
        plan.request(leaf(2)).expect("fits");

        plan.truncate(mark);

        assert_eq!(plan.rows().len(), mark);
        assert_eq!(plan.demanded(), mark);
        assert_eq!(
            plan.request(leaf(0)),
            Some(kept),
            "a surviving demand keeps the id it was given",
        );
        assert_ne!(
            plan.request(leaf(1)),
            Some(kept),
            "a discarded demand is minted afresh, never aliased onto a survivor",
        );
    }
}
