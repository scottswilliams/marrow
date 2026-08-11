//! The site table's capacity policy (red R17).
//!
//! Every operation site is minted through one bounded plan. The plan checks vacant
//! capacity *before* it mints a numeric id, so a fitting site id is always inside
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
    ImageBuildError, ImageDraft, LedgerIdBytes, SemanticPath, SemanticStep, SemanticStepKind,
    SiteDef,
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

/// Red R17, at the boundary an image is refused: a draft whose site demand crosses the
/// cap cannot be encoded, however many demands past it were refused.
///
/// The plan answers every excess demand with the over-policy operand, which carries no id
/// at all, and saturates its logical demand one past the cap; the encoder reads that
/// saturated demand rather than the retained row count, so the image is refused here
/// instead of carrying an aliased operand.
#[test]
fn a_draft_whose_demand_crosses_the_cap_cannot_be_encoded() {
    let mut draft = ImageDraft::new();
    for n in 0..MAX_SITES {
        let _ = draft.request_site(SiteDef::field_leaf(leaf(n)));
    }
    assert!(
        !matches!(draft.encode(), Err(ImageBuildError::TooManySites)),
        "a demand of exactly MAX_SITES fits",
    );

    for n in MAX_SITES..(MAX_SITES + 64) {
        let _ = draft.request_site(SiteDef::field_leaf(leaf(n)));
    }

    assert!(matches!(draft.encode(), Err(ImageBuildError::TooManySites)));
}

/// A demand the plan already retains keeps its operand after the cap is crossed: the
/// crossing is nonblocking, and repeating an earlier reference must not start failing or
/// answer with the refusal.
#[test]
fn a_retained_demand_still_reuses_its_operand_after_the_cap_is_crossed() {
    let mut draft = ImageDraft::new();
    let first = draft.request_site(SiteDef::field_leaf(leaf(0)));
    for n in 1..=MAX_SITES {
        let _ = draft.request_site(SiteDef::field_leaf(leaf(n)));
    }

    let again = draft.request_site(SiteDef::field_leaf(leaf(0)));

    assert_eq!(
        first, again,
        "a repeated reference to one demand returns the operand it was already given",
    );
    assert_ne!(
        first,
        draft.request_site(SiteDef::field_leaf(leaf(MAX_SITES + 1))),
        "an excess demand is refused rather than aliased onto a retained one",
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
        let eager = draft.request_site(first);
        let lazy = draft.request_site(second);

        assert_eq!(
            eager, lazy,
            "a repeated demand reuses the row already minted for it",
        );
        assert_ne!(
            draft.request_site(SiteDef::field_leaf(leaf(0xbeef))),
            eager,
            "the repeated demand appended no second row, so a fresh demand is a fresh row",
        );
    }
}

/// One node addressed by two different operation targets is two demands, not one: the
/// target is part of the key, so a whole-payload site and a group-entry site over one
/// node never collapse into a single row.
#[test]
fn one_node_under_two_targets_is_two_rows() {
    let mut draft = ImageDraft::new();

    let whole = draft.request_site(SiteDef::whole_payload(placement(0x31)));
    let group = draft.request_site(SiteDef::group_entry(placement(0x31)));

    assert_ne!(
        whole, group,
        "two operation targets over one node are two site rows",
    );
}
