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
//! A row retains only its [`OccurrenceSiteDemandKey`] — three owned typed ordinals into
//! the draft's own canonical tables. It retains no semantic path: the path a site encodes
//! to is *projected* from the key at encode time by the one path owner, so the plan
//! cannot hold a path that has drifted from the graph it claims to address, and a row
//! costs a fixed width rather than a per-row heap allocation.
//!
//! Crossing the cap is **nonblocking**: the plan saturates its logical demand count at
//! `MAX_SITES + 1`, records the earliest crossing once, and keeps answering. A demand it
//! already retains still resolves to the id it was given, so a repeated reference never
//! begins to fail; every other post-cap demand is refused an id rather than aliasing a
//! fitting one. The encoder projects the saturated logical count through the Sites bound,
//! so an image whose demand crossed the cap is refused there as it always was.

use std::collections::HashMap;

use crate::bounds::MAX_SITES;
use crate::draft::{DraftIdentity, ImageBuildError, RowStamp, SiteId};
use crate::product::{BindRefusal, BoundDemand, OccurrenceSiteDemandKey};

/// A validated site demand, ready to be requested from the plan that validated it.
///
/// It carries the complete demand key plus the stable draft identity and the exact live
/// occurrence and declaration rows the binder validated it against. There is no public or
/// raw key-to-handle constructor and no rebinding step: the sole mint is
/// [`crate::ImageDraft::bind_occurrence_site`], so a handle is evidence that this draft
/// answered for this place, not a value a caller can assert.
///
/// It is `Clone` but not `Copy`, for the same reason a selector is not.
#[doc(hidden)]
#[derive(Clone)]
pub struct OccurrenceSiteHandle {
    draft: DraftIdentity,
    demand: BoundDemand,
}

impl OccurrenceSiteHandle {
    pub(crate) fn new(draft: DraftIdentity, demand: BoundDemand) -> Self {
        Self { draft, demand }
    }

    pub(crate) fn draft(&self) -> DraftIdentity {
        self.draft
    }

    pub(crate) fn demand(&self) -> BoundDemand {
        self.demand
    }
}

impl std::fmt::Debug for OccurrenceSiteHandle {
    /// One fixed marker: a handle's ordinals and row stamps are the authority it carries.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("occurrence site handle")
    }
}

/// A site binding, request, or code insertion that the plan refused.
///
/// It is one opaque invariant to every consumer outside this crate: there is no variant,
/// field, constructor, accessor, raw ordinal, identity, token, or source payload, and its
/// rendered description is one fixed non-sensitive sentence that allocates nothing. The
/// image owner matches its private cases exhaustively and tests them; the compiler maps
/// the whole type to one compiler-invariant failure.
///
/// Crossing the site cap is **not** this error: an over-policy operand is a successful
/// opaque state of the operand, and the encoder refuses that image through the Sites
/// bound.
#[doc(hidden)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SitePlanStateError(SitePlanState);

/// What the plan refused. Private, fixed, and allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SitePlanState {
    /// A selector, handle, or operand was published by another draft's plan.
    WrongPlan,
    /// The row a selector, handle, or operand names is gone, or a later row reused its
    /// ordinal.
    StaleBinding,
    /// The occurrence, path, and target do not name a place of this graph.
    InvalidDemand,
}

impl SitePlanStateError {
    pub(crate) fn new(state: SitePlanState) -> Self {
        Self(state)
    }
}

impl From<BindRefusal> for SitePlanStateError {
    fn from(refusal: BindRefusal) -> Self {
        Self(match refusal {
            BindRefusal::WrongPlan => SitePlanState::WrongPlan,
            BindRefusal::StaleBinding => SitePlanState::StaleBinding,
            BindRefusal::InvalidDemand => SitePlanState::InvalidDemand,
        })
    }
}

/// One fixed description for every case. Which case was refused is a producer-side
/// invariant the image owner tests directly; publishing it here would make the private
/// discriminant a public one by another spelling.
const SITE_PLAN_STATE_DESCRIPTION: &str = "the draft did not answer for this operation site";

impl std::fmt::Debug for SitePlanStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(SITE_PLAN_STATE_DESCRIPTION)
    }
}

impl std::fmt::Display for SitePlanStateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(SITE_PLAN_STATE_DESCRIPTION)
    }
}

impl std::error::Error for SitePlanStateError {}

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
/// Its equality and `Debug` are over the **logical ordinal only**. This is the public
/// instruction IR's operand, so what two instructions mean is the number that reaches the
/// wire; the provenance it carries privately is checked where a site is inserted into a
/// function, never smuggled into instruction semantics.
///
/// **Temporary.** It is the existing draft instruction IR's operand, not a parallel
/// lowering IR, and it is replaced wholesale when the planned site reference lands under
/// an admitted transaction.
#[derive(Clone)]
pub struct LegacyDraftSiteOperand {
    kind: LegacyDraftSiteOperandKind,
    provenance: SiteOperandProvenance,
}

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

/// Where one operand came from: the draft and plan that answered, the validated demand,
/// and the exact live row the answer stands on — the site row for a fitting operand, the
/// policy receipt for an over-policy one.
///
/// It is private and takes no part in equality or `Debug`. It is what
/// [`crate::ImageDraft::add_function`] validates before a code vector is appended, so an
/// operand transplanted from another draft, or one whose row was discarded and its
/// ordinal deterministically reused, cannot enter a function body.
#[derive(Clone, Copy)]
struct SiteOperandProvenance {
    draft: DraftIdentity,
    demand: BoundDemand,
    row: RowStamp,
}

impl LegacyDraftSiteOperand {
    /// The wire ordinal this operand encodes to.
    ///
    /// An over-policy operand has none. The encoder's Sites bound reads the plan's
    /// saturated logical demand and refuses such an image before any body byte is
    /// written, so this arm is unreachable through [`crate::ImageDraft::encode`]; it is
    /// spelled as a refusal rather than a stand-in number so that no non-site value can
    /// ever be written into a site operand's two bytes.
    pub(crate) fn encodable(&self) -> Result<u16, ImageBuildError> {
        match self.kind {
            LegacyDraftSiteOperandKind::Fitting(id) => Ok(id.index()),
            LegacyDraftSiteOperandKind::OverPolicy => Err(ImageBuildError::TooManySites),
        }
    }
}

/// Equality is over the logical site ordinal, not over which plan minted it.
impl PartialEq for LegacyDraftSiteOperand {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for LegacyDraftSiteOperand {}

/// A fitting operand renders as the logical site number it carries, so an instruction's
/// `Debug` reads exactly as it did when the operand was a bare `u16`. An over-policy
/// operand renders one fixed marker: there is no number to show, and inventing one would
/// be the very aliasing the type exists to prevent.
impl std::fmt::Debug for LegacyDraftSiteOperand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            LegacyDraftSiteOperandKind::Fitting(id) => write!(f, "{}", id.index()),
            LegacyDraftSiteOperandKind::OverPolicy => f.write_str("over-policy"),
        }
    }
}

/// The record that a plan's demand crossed `MAX_SITES`. The plan retains no row or key
/// past the crossing, so this is what remains of every demand beyond it.
///
/// It is a typed fact rather than a flag: "this plan's demand exceeded its capacity" is a
/// state the encoder reads and the draft cannot un-observe, and giving it a name keeps a
/// bare boolean from standing in for it. One receipt is recorded, at the earliest
/// crossing; later refusals do not replace it, and it carries the stamp an over-policy
/// operand stands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SitePolicyReceipt {
    stamp: RowStamp,
}

/// One retained site row: the demand it answers and the stamp that distinguishes it from
/// a later row deterministically reusing its ordinal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SiteRow {
    key: OccurrenceSiteDemandKey,
    stamp: RowStamp,
}

impl SiteRow {
    pub(crate) fn key(&self) -> OccurrenceSiteDemandKey {
        self.key
    }
}

/// The bounded site demand plan.
#[derive(Debug, Default, Clone)]
pub(crate) struct SiteDemandPlan {
    /// The retained site rows, in emission order. A row's index is its [`SiteId`].
    rows: Vec<SiteRow>,
    /// The retained demands, so a repeated reference to one place returns the id already
    /// minted for it. The rows vector stays the sole table authority and emission order;
    /// this only makes the table carry a site per *demanded* place rather than one per
    /// declared graph node.
    retained: HashMap<OccurrenceSiteDemandKey, SiteId>,
    /// The earliest crossing, recorded once. With the rows it is the whole logical demand
    /// count: a plan retains exactly one row per fitting demand, and the receipt stands for
    /// every demand past the cap, so no separate counter can drift from the table.
    receipt: Option<SitePolicyReceipt>,
}

impl SiteDemandPlan {
    /// Mint-or-return the operand answering the validated `demand`.
    ///
    /// The over-policy operand is the answer when the plan has no vacant capacity and
    /// does not already retain this demand, so there is no id it can give that would not
    /// alias a fitting one.
    pub(crate) fn request(
        &mut self,
        draft: DraftIdentity,
        demand: BoundDemand,
        stamp: RowStamp,
    ) -> LegacyDraftSiteOperand {
        let key = demand.key();
        if let Some(existing) = self.retained.get(&key) {
            let row = self.rows[usize::from(existing.index())].stamp;
            return LegacyDraftSiteOperand {
                kind: LegacyDraftSiteOperandKind::Fitting(*existing),
                provenance: SiteOperandProvenance { draft, demand, row },
            };
        }
        // Vacant capacity is checked before any numeric id is minted, so the fitting range
        // stays exactly `0..MAX_SITES` and the conversion below cannot narrow a length
        // into an id that aliases one.
        if self.rows.len() >= MAX_SITES {
            return self.saturate(draft, demand, stamp);
        }
        let Ok(ordinal) = u16::try_from(self.rows.len()).map(SiteId::from_ordinal) else {
            return self.saturate(draft, demand, stamp);
        };
        self.rows.push(SiteRow { key, stamp });
        self.retained.insert(key, ordinal);
        LegacyDraftSiteOperand {
            kind: LegacyDraftSiteOperandKind::Fitting(ordinal),
            provenance: SiteOperandProvenance {
                draft,
                demand,
                row: stamp,
            },
        }
    }

    /// Record the crossing and refuse an id. The logical count saturates at
    /// `MAX_SITES + 1` — one past the cap is all the encoder's bound needs to read, and
    /// counting every excess demand would retain unbounded work for a refused image.
    fn saturate(
        &mut self,
        draft: DraftIdentity,
        demand: BoundDemand,
        stamp: RowStamp,
    ) -> LegacyDraftSiteOperand {
        let receipt = *self.receipt.get_or_insert(SitePolicyReceipt { stamp });
        LegacyDraftSiteOperand {
            kind: LegacyDraftSiteOperandKind::OverPolicy,
            provenance: SiteOperandProvenance {
                draft,
                demand,
                row: receipt.stamp,
            },
        }
    }

    /// Whether `operand` still stands on this plan: it was minted by this draft, and the
    /// exact row it was minted against is still live.
    ///
    /// A clone of a valid same-plan operand stays valid; an operand whose site row or
    /// receipt was discarded is stale even when the ordinal it names was afterwards
    /// re-minted, because the re-minted row carries a fresh stamp.
    pub(crate) fn validate(
        &self,
        draft: DraftIdentity,
        operand: &LegacyDraftSiteOperand,
    ) -> Result<(), SitePlanState> {
        if operand.provenance.draft != draft {
            return Err(SitePlanState::WrongPlan);
        }
        match operand.kind {
            LegacyDraftSiteOperandKind::Fitting(id) => {
                let row = self
                    .rows
                    .get(usize::from(id.index()))
                    .ok_or(SitePlanState::StaleBinding)?;
                if row.stamp != operand.provenance.row {
                    return Err(SitePlanState::StaleBinding);
                }
                if row.key != operand.provenance.demand.key() {
                    return Err(SitePlanState::InvalidDemand);
                }
                Ok(())
            }
            LegacyDraftSiteOperandKind::OverPolicy => match self.receipt {
                Some(receipt) if receipt.stamp == operand.provenance.row => Ok(()),
                _ => Err(SitePlanState::StaleBinding),
            },
        }
    }

    /// The retained site rows, in emission order.
    pub(crate) fn rows(&self) -> &[SiteRow] {
        &self.rows
    }

    /// The logical demand count, saturating at `MAX_SITES + 1`. The encoder's Sites bound
    /// reads this rather than the row count: the plan refuses to mint past its capacity, so
    /// the row count can never exceed the bound and reading it would disable the check.
    pub(crate) fn demanded(&self) -> usize {
        self.rows.len() + usize::from(self.receipt.is_some())
    }

    /// The earliest crossing this plan has recorded, if any.
    pub(crate) fn receipt(&self) -> Option<SitePolicyReceipt> {
        self.receipt
    }

    /// Discard every row appended after `rows` and restore `receipt` as the plan's policy
    /// state, dropping exactly the removed rows' demand keys.
    ///
    /// The purge is reconstructed from the removed rows, so it is proportional to the
    /// discarded suffix rather than to the whole table.
    ///
    /// `receipt` is the exact receipt the plan held when the suffix began. A crossing that
    /// happened before it keeps its identity, so an operand minted over-policy before the
    /// suffix stays valid; a crossing first recorded inside the suffix is cleared with the
    /// demands that caused it, so a discarded pass cannot leave the plan refusing sites it
    /// has capacity for, and an over-policy operand minted inside it is stale.
    pub(crate) fn rewind(&mut self, rows: usize, receipt: Option<SitePolicyReceipt>) {
        for row in self.rows.drain(rows..) {
            self.retained.remove(&row.key);
        }
        self.receipt = receipt;
    }
}
