//! The ledger kind tag space is frozen, and `marrow-image` mirrors it by hand.
//!
//! `marrow-image`'s durable contract identity encodes each ledger reference as
//! `IDREF(kind, id)`, where the kind byte is a hand-written constant
//! (`marrow-image/src/durable_id.rs`, `IDREF_APPLICATION`..`IDREF_INDEX`) that must
//! equal this crate's [`IdentityKind::tag`]. The two are coupled by nothing but
//! agreement: `marrow-project` deliberately does not depend on `marrow-image`, and
//! `marrow-image` deliberately does not depend on `marrow-project`, so no type or
//! constant reaches across.
//!
//! A silent divergence would not fail to compile. It would change the contract-ID
//! preimage of every durable image while the producer and the independent verifier
//! still agreed with each other, so no test that round-trips one build would notice
//! — the identities would simply mean something else than the ledger says.
//!
//! This test is one half of the drift gate: the tags are restated here as literals,
//! so changing [`IdentityKind::tag`] fails here. The other half is the
//! literal-stripping absence gate over `marrow-image`'s constants
//! (`marrow-image/tests/identity_tag_mirror.rs`), so changing the mirror fails
//! there. Neither half may be relaxed to accommodate the other: a real kind-space
//! change edits the ledger, the mirror, and both gates in one transaction.

use marrow_project::IdentityKind;

/// The frozen tag of every ledger kind, restated as a literal table in the same
/// order as `IdentityKind::ALL`. The right-hand column names the `marrow-image`
/// constant that must carry the same byte.
const FROZEN_TAGS: &[(IdentityKind, u8, &str)] = &[
    (IdentityKind::Application, 0, "IDREF_APPLICATION"),
    (IdentityKind::Product, 1, "IDREF_PRODUCT"),
    (IdentityKind::Field, 2, "IDREF_FIELD"),
    (IdentityKind::Root, 3, "IDREF_ROOT"),
    (IdentityKind::Key, 4, "IDREF_KEY"),
    (IdentityKind::Sum, 5, "IDREF_SUM"),
    (IdentityKind::Member, 6, "IDREF_MEMBER"),
    (IdentityKind::Group, 7, "IDREF_GROUP"),
    (IdentityKind::Index, 8, "IDREF_INDEX"),
];

#[test]
fn every_ledger_kind_carries_its_frozen_tag() {
    for (kind, tag, mirror) in FROZEN_TAGS {
        assert_eq!(
            kind.tag(),
            *tag,
            "{kind:?} carries the frozen tag {tag}, mirrored by marrow-image's {mirror}",
        );
    }
}

/// The table above covers the whole kind space: a new kind must be given a frozen
/// tag and a mirror constant, not left to inherit one by position.
#[test]
fn the_frozen_table_covers_every_kind_exactly_once() {
    assert_eq!(
        FROZEN_TAGS.len(),
        IdentityKind::ALL.len(),
        "every IdentityKind appears in the frozen tag table",
    );
    for (index, kind) in IdentityKind::ALL.iter().enumerate() {
        assert_eq!(
            FROZEN_TAGS[index].0, *kind,
            "the frozen tag table is in IdentityKind::ALL order",
        );
    }
}

/// Tags are distinct, so no two ledger kinds can be confused in an `IDREF`
/// preimage.
#[test]
fn frozen_tags_are_pairwise_distinct() {
    for (index, (kind, tag, _)) in FROZEN_TAGS.iter().enumerate() {
        for (other_kind, other_tag, _) in &FROZEN_TAGS[index + 1..] {
            assert_ne!(
                tag, other_tag,
                "{kind:?} and {other_kind:?} must not share a tag",
            );
        }
    }
}
