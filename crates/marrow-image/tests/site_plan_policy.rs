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
