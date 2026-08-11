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
use crate::draft::{ImageBuildError, SiteDef, SiteId};
use crate::semantic::{SemanticPath, SemanticTarget};

/// The operation-site operand a draft instruction carries.
///
/// It is opaque and has no public constructor, field, variant, raw-id accessor,
/// `Default`, or `From`: the bounded [`SiteDemandPlan`] is its sole mint, so an
/// instruction cannot name a site no plan answered. Before this type the `Dur*`
/// instructions carried a bare `u16`, which any producer could write by hand and which
/// forced a refused site to be smuggled through the same numeric channel as a real one.
///
/// It is `Clone` but deliberately not `Copy`: a site operand is a minted answer, and
/// copying one implicitly is how a carrier ends up holding a site it never requested.
///
/// **Temporary.** It is the existing draft instruction IR's operand, not a parallel
/// lowering IR, and it is replaced wholesale when the planned site reference lands under
/// an admitted transaction.
#[derive(Clone, PartialEq, Eq)]
pub struct LegacyDraftSiteOperand(LegacyDraftSiteOperandKind);

/// What a site operand stands for: the id the plan minted, or the plan's refusal.
///
/// Over-policy is a successful opaque operand state rather than an error. The crossing
/// is nonblocking — lowering continues and the encoder refuses the image through the
/// Sites bound — but there is no id that would not alias a fitting site, so none is
/// carried. Keeping the refusal in the operand's own type is what makes it impossible to
/// compare or encode as if it were a site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyDraftSiteOperandKind {
    Fitting(SiteId),
    OverPolicy,
}

impl LegacyDraftSiteOperand {
    fn fitting(id: SiteId) -> Self {
        Self(LegacyDraftSiteOperandKind::Fitting(id))
    }

    fn over_policy() -> Self {
        Self(LegacyDraftSiteOperandKind::OverPolicy)
    }

    /// The wire ordinal this operand encodes to.
    ///
    /// An over-policy operand has none. The encoder's Sites bound reads the plan's
    /// saturated logical demand and refuses such an image before any body byte is
    /// written, so this arm is unreachable through [`crate::ImageDraft::encode`]; it is
    /// spelled as a refusal rather than a stand-in number so that no non-site value can
    /// ever be written into a site operand's two bytes.
    pub(crate) fn encodable(&self) -> Result<u16, ImageBuildError> {
        match self.0 {
            LegacyDraftSiteOperandKind::Fitting(id) => Ok(id.index()),
            LegacyDraftSiteOperandKind::OverPolicy => Err(ImageBuildError::TooManySites),
        }
    }
}

/// A fitting operand renders as the logical site number it carries, so an instruction's
/// `Debug` reads exactly as it did when the operand was a bare `u16`. An over-policy
/// operand renders one fixed marker: there is no number to show, and inventing one would
/// be the very aliasing the type exists to prevent.
impl std::fmt::Debug for LegacyDraftSiteOperand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            LegacyDraftSiteOperandKind::Fitting(id) => write!(f, "{}", id.index()),
            LegacyDraftSiteOperandKind::OverPolicy => f.write_str("over-policy"),
        }
    }
}

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
    /// Mint-or-return the operand answering `def`.
    ///
    /// The over-policy operand is the answer when the plan has no vacant capacity and
    /// does not already retain this demand, so there is no id it can give that would not
    /// alias a fitting one.
    pub(crate) fn request(&mut self, def: SiteDef) -> LegacyDraftSiteOperand {
        let key = (def.path.clone(), def.target);
        if let Some(existing) = self.retained.get(&key) {
            return LegacyDraftSiteOperand::fitting(*existing);
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
        LegacyDraftSiteOperand::fitting(ordinal)
    }

    /// Record the crossing and refuse an id. The logical count saturates at
    /// `MAX_SITES + 1` — one past the cap is all the encoder's bound needs to read, and
    /// counting every excess demand would retain unbounded work for a refused image.
    fn saturate(&mut self) -> LegacyDraftSiteOperand {
        self.receipt.get_or_insert(SitePolicyReceipt);
        LegacyDraftSiteOperand::over_policy()
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
            if let Ok(id) = plan.request(leaf(n)).encodable() {
                minted.push(id);
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
                .encodable()
                .expect("every demand below the cap fits");
        }
        assert_eq!(
            plan.demanded(),
            MAX_SITES,
            "no receipt is recorded at exactly MAX_SITES, so the demand still fits",
        );

        assert!(
            plan.request(leaf(MAX_SITES)).encodable().is_err(),
            "the first excess is refused"
        );
        assert_eq!(
            plan.demanded(),
            MAX_SITES + 1,
            "the crossing is recorded and the logical count saturates one past the cap",
        );

        for n in (MAX_SITES + 1)..(MAX_SITES + 64) {
            assert!(plan.request(leaf(n)).encodable().is_err());
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
        let first = plan.request(leaf(0));
        for n in 1..=MAX_SITES {
            let _ = plan.request(leaf(n));
        }
        assert_eq!(
            plan.request(leaf(0)),
            first,
            "a repeated reference returns the id already minted for it",
        );
    }

    /// Discarding a suffix drops exactly that suffix's demands, and a demand that survived
    /// keeps its id — the purge cannot remove a surviving row's key.
    #[test]
    fn discarding_a_suffix_keeps_every_surviving_demand_resolvable() {
        let mut plan = SiteDemandPlan::default();
        let kept = plan.request(leaf(0));
        let mark = plan.rows().len();
        plan.request(leaf(1));
        plan.request(leaf(2));

        plan.truncate(mark);

        assert_eq!(plan.rows().len(), mark);
        assert_eq!(plan.demanded(), mark);
        assert_eq!(
            plan.request(leaf(0)),
            kept,
            "a surviving demand keeps the id it was given",
        );
        assert_ne!(
            plan.request(leaf(1)),
            kept,
            "a discarded demand is minted afresh, never aliased onto a survivor",
        );
    }

    /// A fitting operand renders as its logical site number, so an instruction's `Debug`
    /// reads as it did when the operand was a bare `u16` and every golden that shows an
    /// instruction stays legible.
    #[test]
    fn a_fitting_operand_debugs_as_its_logical_site_number() {
        let mut plan = SiteDemandPlan::default();

        assert_eq!(format!("{:?}", plan.request(leaf(0))), "0");
        assert_eq!(format!("{:?}", plan.request(leaf(1))), "1");
        assert_eq!(
            format!("{:?}", plan.request(leaf(0))),
            "0",
            "a repeated demand renders the id it was already given",
        );
    }

    /// An over-policy operand renders one fixed marker and no number: there is no site to
    /// name, and rendering a stand-in would read as a real site in every log and panic
    /// message that shows an instruction.
    #[test]
    fn an_over_policy_operand_debugs_as_a_redacted_marker() {
        let mut plan = SiteDemandPlan::default();
        for n in 0..MAX_SITES {
            let _ = plan.request(leaf(n));
        }

        let refused = plan.request(leaf(MAX_SITES));

        assert_eq!(format!("{refused:?}"), "over-policy");
        assert!(refused.encodable().is_err());
        assert_eq!(
            refused,
            plan.request(leaf(MAX_SITES + 1)),
            "every refusal is the one marker, so no two refusals are distinguishable",
        );
    }

    /// Equality is over the logical site ordinal, not over which plan minted it. The
    /// operand is the instruction IR's public operand, so its equality is the equality of
    /// the number that reaches the wire; provenance is checked where a site is requested,
    /// never smuggled into what two instructions mean.
    #[test]
    fn operands_with_one_ordinal_are_equal_across_independently_built_plans() {
        let mut first = SiteDemandPlan::default();
        let mut second = SiteDemandPlan::default();

        let from_first = first.request(leaf(0));
        let _ = second.request(leaf(7));
        let from_second = second.request(leaf(9));

        assert_eq!(
            from_first,
            second.request(leaf(7)),
            "the same ordinal from another plan is the same operand",
        );
        assert_ne!(
            from_first, from_second,
            "a different ordinal is a different operand",
        );
    }
}
