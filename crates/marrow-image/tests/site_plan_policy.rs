//! The site table's capacity policy (red R17).
//!
//! Every operation site is minted through one bounded plan. The plan checks vacant
//! capacity *before* it mints a numeric id, so a fitting `SiteId` is always inside
//! `0..=8191`; the first unique demand past the cap saturates the logical count at
//! `MAX_SITES + 1` and records one earliest policy receipt, while an already-retained
//! demand still reuses the id it was given.
//!
//! Before this, `add_site` was a bare push whose id was `self.sites.len() as u16`, with
//! the bound seen only at `encode()`. A producer could push past `u16::MAX`, receive a
//! wrapped id, and embed the aliased id in emitted instruction operands — two distinct
//! durable nodes silently sharing one site operand, discovered nowhere.

use marrow_image::bounds::MAX_SITES;
use marrow_image::{
    ImageDraft, LedgerIdBytes, SemanticPath, SemanticStep, SemanticStepKind, SiteDef,
};

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PLACEMENT_ID: [u8; 16] = [0x0b; 16];

/// A distinct field-leaf path below one root, seeded by `n`, so every request below is a
/// distinct `(node, target)` demand.
fn leaf(n: usize) -> SemanticPath {
    let mut bytes = [0x50u8; 16];
    bytes[0] = u8::try_from(n & 0xff).expect("masked to one byte");
    bytes[1] = u8::try_from((n >> 8) & 0xff).expect("masked to one byte");
    bytes[2] = u8::try_from((n >> 16) & 0xff).expect("masked to one byte");
    SemanticPath::root(
        LedgerIdBytes::from_bytes(APPLICATION_ID),
        LedgerIdBytes::from_bytes(PLACEMENT_ID),
    )
    .child(SemanticStep::new(
        SemanticStepKind::Field,
        LedgerIdBytes::from_bytes(bytes),
    ))
}

/// Every fitting site id is inside `0..=8191`, and no id past the cap ever aliases a
/// fitting one. At `MAX_SITES + 1` unique demands the plan must not have handed out a
/// duplicate or wrapped ordinal.
#[test]
fn a_fitting_site_id_is_never_wrapped_or_aliased() {
    let mut draft = ImageDraft::new();
    let mut fitting = Vec::new();
    for n in 0..=MAX_SITES {
        if let Some(id) = draft.request_site(SiteDef::field_leaf(leaf(n))) {
            fitting.push(id.index());
        }
    }

    assert_eq!(
        fitting.len(),
        MAX_SITES,
        "exactly the fitting demands receive an id",
    );
    assert!(
        fitting.iter().all(|id| usize::from(*id) < MAX_SITES),
        "a minted site id is inside the table's capacity",
    );
    let mut sorted = fitting.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        fitting.len(),
        "no two distinct durable demands share one site id",
    );
}

/// Far past the cap the plan keeps refusing rather than wrapping. `u16::MAX + 2` unique
/// demands is the shape that produced `SiteId(0)` for the 65,536th site.
#[test]
fn requests_far_past_the_cap_never_wrap_into_a_fitting_id() {
    let mut draft = ImageDraft::new();
    let mut minted = 0usize;
    for n in 0..(usize::from(u16::MAX) + 2) {
        if draft.request_site(SiteDef::field_leaf(leaf(n))).is_some() {
            minted += 1;
        }
    }
    assert_eq!(
        minted, MAX_SITES,
        "the plan mints exactly its capacity and refuses every later unique demand",
    );
}

/// A demand the plan already retains keeps its id after the cap is crossed: the crossing
/// is nonblocking, and repeating an earlier reference must not start failing.
#[test]
fn a_retained_demand_still_reuses_its_id_after_the_cap_is_crossed() {
    let mut draft = ImageDraft::new();
    let first = draft
        .request_site(SiteDef::field_leaf(leaf(0)))
        .expect("the first demand fits");
    for n in 1..=MAX_SITES {
        let _ = draft.request_site(SiteDef::field_leaf(leaf(n)));
    }
    let again = draft
        .request_site(SiteDef::field_leaf(leaf(0)))
        .expect("a retained demand still resolves after the cap is crossed");
    assert_eq!(
        first.index(),
        again.index(),
        "a repeated reference to one demand returns the id it was already given",
    );
}

/// The keyed placement `n` names, for the eager/lazy same-key pair below.
fn placement(n: u8) -> SemanticPath {
    SemanticPath::root(
        LedgerIdBytes::from_bytes(APPLICATION_ID),
        LedgerIdBytes::from_bytes([n; 16]),
    )
}

/// Red R18: an eagerly minted row followed by a demand-lazy request on the same
/// `(node, target)` is **one** row, and the second request reuses the first row's id — in
/// either order.
///
/// The eager per-node sites a durable graph emits when it is built and the field-leaf
/// sites the lowerer allocates on first reference were two mint paths: the eager one
/// appended a row the demand map never saw, so a later request on the same
/// `(node, target)` appended a second row for one node, and the two rows were separate
/// operands for one place. Both now enter the one plan, which answers a retained demand
/// with the id it already minted.
///
/// The two paths are disjoint on every production graph — an eager demand's terminal step
/// is a placement, group, or index node and its target is never `FieldLeaf` — so unifying
/// them re-mints nothing and no image byte moves. This pins the behavior that makes that
/// safe rather than the disjointness, which a future graph shape could narrow.
#[test]
fn one_demand_minted_eagerly_then_lazily_is_one_row_with_one_id() {
    for (first, second) in [
        (
            SiteDef::whole_payload(placement(0x21)) as SiteDef,
            SiteDef::whole_payload(placement(0x21)),
        ),
        (SiteDef::field_leaf(leaf(1)), SiteDef::field_leaf(leaf(1))),
        (
            SiteDef::group_entry(placement(0x22)),
            SiteDef::group_entry(placement(0x22)),
        ),
        (
            SiteDef::index_scan(placement(0x23)),
            SiteDef::index_scan(placement(0x23)),
        ),
        (
            SiteDef::index_lookup(placement(0x24)),
            SiteDef::index_lookup(placement(0x24)),
        ),
    ] {
        let mut draft = ImageDraft::new();
        let eager = draft.request_site(first).expect("the first demand fits");
        let lazy = draft
            .request_site(second)
            .expect("the repeated demand fits");

        assert_eq!(
            eager.index(),
            lazy.index(),
            "a repeated demand reuses the row already minted for it",
        );
        let next = draft
            .request_site(SiteDef::field_leaf(leaf(0xbeef)))
            .expect("a fresh demand fits");
        assert_eq!(
            next.index(),
            1,
            "the repeated demand appended no second row, so the next id is the second",
        );
    }
}

/// One node addressed by two different operation targets is two demands, not one: the
/// target is part of the key, so a whole-payload site and a group-entry site over one
/// node never collapse into a single row.
#[test]
fn one_node_under_two_targets_is_two_rows() {
    let mut draft = ImageDraft::new();

    let whole = draft
        .request_site(SiteDef::whole_payload(placement(0x31)))
        .expect("the whole-payload demand fits");
    let group = draft
        .request_site(SiteDef::group_entry(placement(0x31)))
        .expect("the group-entry demand fits");

    assert_ne!(
        whole.index(),
        group.index(),
        "two operation targets over one node are two site rows",
    );
}
