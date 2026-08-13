//! Which declared bound a durable image actually reaches first.
//!
//! The Roots ceiling is reachable from a graph that is otherwise well inside every other
//! bound, and the encoder selects it as the refusal candidate with a complete provisional
//! graph and no image output allocated.
//!
//! The corpus is the frozen clean-Roots corpus: Product `R` with the one field
//! `required v: int`, no keys, indexes, or tombstones, and `N` distinct keyless roots
//! `r0`..`r{N-1}` occurring over it. Each root's whole-payload site is eager, so the
//! census of one such image is `N` root occurrences, one record type, `N + 2` strings
//! (`R`, `v`, and one name per root) and `N` sites.
//!
//! The corpus also carries the two gate-asserted byte totals for `MAX_ROOTS + 1` and
//! `MAX_ROOTS + 2` roots, recomputed here. Neither total is ever emitted: the real
//! Roots candidate wins first, and the point of the pair is exactly that an image nobody
//! can encode still has an exact size that a bound change would move.

use marrow_image::bounds::{MAX_IMAGE_BYTES, MAX_ROOTS, MAX_SITES, MAX_STRINGS, MAX_TYPES};
use marrow_image::{
    DeclarationMemberDef, DeclarationMemberShape, FieldDef, ImageBuildError, ImageDraft, ImageType,
    LedgerIdBytes, RecordTypeDef, RootOccurrenceDef, Scalar, SemanticTarget,
};

#[path = "../../marrow-image/tests/common/site_seam.rs"]
mod site_seam;
use site_seam::site;

#[path = "../../marrow-image/tests/common/admitted_plan.rs"]
mod admitted_plan;
use admitted_plan::admitted_plan;

#[path = "common/admitted.rs"]
mod admitted_helper;
use admitted_helper::admitted;

const APPLICATION_ID: [u8; 16] = [0x0a; 16];
const PRODUCT_ID: [u8; 16] = [0x0d; 16];
const FIELD_ID: [u8; 16] = [0x0e; 16];

/// The placement identity of root `n`. Each root is its own occurrence, so each carries
/// its own placement ledger id.
fn placement(n: usize) -> LedgerIdBytes {
    let mut bytes = [0x20u8; 16];
    bytes[0] = (n & 0xff) as u8;
    bytes[1] = ((n >> 8) & 0xff) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// The declaration-member identity of the `n`th declared field of the wide corpus below.
/// It is a distinct base byte from [`placement`], so no member can collide with a root.
fn member_id(n: usize) -> LedgerIdBytes {
    let mut bytes = [0x60u8; 16];
    bytes[0] = (n & 0xff) as u8;
    bytes[1] = ((n >> 8) & 0xff) as u8;
    bytes[2] = ((n >> 16) & 0xff) as u8;
    LedgerIdBytes::from_bytes(bytes)
}

/// The clean-Roots corpus at `roots` occurrences: one Product declaring `required v: int`,
/// and `roots` keyless roots over it, each with its eager whole-payload site.
fn roots_corpus(roots: usize) -> ImageDraft {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let resource = draft.intern_string("R").expect("a within-domain mint");
    let field = draft.intern_string("v").expect("a within-domain mint");
    let record = draft
        .add_record_type(RecordTypeDef {
            name: resource,
            fields: vec![FieldDef {
                name: field,
                ty: ImageType::scalar(Scalar::Int),
                required: true,
            }],
        })
        .expect("a within-domain mint");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let value = draft.value_scalar(Scalar::Int);
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            vec![DeclarationMemberDef {
                parent: None,
                shape: DeclarationMemberShape::Field {
                    id: LedgerIdBytes::from_bytes(FIELD_ID),
                    required: true,
                    value,
                },
            }],
        )
        .expect("a well-formed declaration");
    for n in 0..roots {
        let name = draft
            .intern_string(&format!("r{n}"))
            .expect("a within-domain mint");
        let admitted = draft
            .add_root_occurrence(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: placement(n),
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        site(
            &mut draft,
            admitted.occurrence(),
            admitted.placement_path(),
            SemanticTarget::WholePayload,
        );
    }
    draft.commit();
    draft_owner
}

/// The exact framed size of the corpus at `roots` occurrences, encoded through the
/// production encoder. It is the only size authority here: nothing re-implements the
/// container grammar to predict one.
fn framed(roots: usize) -> usize {
    roots_corpus(roots)
        .encode()
        .expect("the corpus fits every bound at this width")
        .bytes
        .len()
}

/// At `MAX_ROOTS + 1` roots the corpus is inside every other declared bound —
/// one record type, `MAX_ROOTS + 3` strings, `MAX_ROOTS + 1` sites, and a framed size well
/// under `MAX_IMAGE_BYTES` — so Roots is the only bound it crosses, and the encoder
/// answers with it.
#[test]
fn the_roots_ceiling_is_reachable_and_is_the_candidate() {
    const OVER: usize = MAX_ROOTS + 1;
    // Every other table this corpus touches has room at that width, so Roots is the only
    // bound it can cross.
    const _: () = assert!(OVER + 2 <= MAX_STRINGS && OVER <= MAX_SITES && MAX_TYPES >= 1);
    let over = OVER;

    let draft = roots_corpus(over);
    assert_eq!(
        draft.encode().expect_err("one root past the ceiling"),
        ImageBuildError::TooManyRoots,
        "Roots is the candidate, not a later byte or table bound",
    );
    // The graph the candidate is reported over is complete rather than truncated at the
    // ceiling: the same corpus one root smaller encodes and verifies whole.
    let below = roots_corpus(MAX_ROOTS).encode().expect("MAX_ROOTS fits");
    assert_eq!(
        marrow_verify::verify(&below.bytes)
            .expect("verifies")
            .roots()
            .len(),
        MAX_ROOTS,
    );
}

/// The census of the corpus, read back from the **independent verifier's** own
/// reconstruction rather than from the producer that wrote it: one occurrence per declared
/// root, one record type however many roots project it, and exactly one eager whole-payload
/// site per root.
///
/// The string census — `R`, `v`, and one name per root — is the corpus construction's own
/// statement: the verified image publishes no string-pool accessor, so it is asserted here
/// as the width it must stay under rather than as a count read back.
#[test]
fn the_corpus_census_is_exact() {
    let at = MAX_ROOTS;
    let image = marrow_verify::verify(&roots_corpus(at).encode().expect("fits").bytes)
        .expect("the corpus verifies independently");
    assert_eq!(image.roots().len(), at, "one occurrence per declared root");
    assert_eq!(
        image.record_types().len(),
        1,
        "one Product, one record type"
    );
    assert_eq!(
        image.sites().len(),
        at,
        "one eager whole-payload site per root"
    );
    const _: () = assert!(
        MAX_ROOTS + 2 <= MAX_STRINGS,
        "`R`, `v`, and one name per root"
    );
}

/// The two gate-asserted byte totals for the corpus one and two roots past the
/// Roots ceiling, recomputed here rather than carried forward.
///
/// Neither image can be encoded — Roots refuses both — so the totals are measured the only
/// honest way: from the production encoder at the two largest widths that *do* encode. Root
/// names `r1000`..`r4098` are all five characters, so the corpus is exactly affine in this
/// regime and one measured per-root step carries to both widths. The step is measured, not
/// assumed: it is the difference the encoder actually produced.
#[test]
fn the_framed_totals_past_the_roots_ceiling_reproduce() {
    let at_ceiling = framed(MAX_ROOTS);
    let one_below = framed(MAX_ROOTS - 1);
    let step = at_ceiling - one_below;

    let over_one = at_ceiling + step;
    let over_two = at_ceiling + 2 * step;
    assert_eq!(
        (at_ceiling, step),
        (429_140, 105),
        "the corpus at the ceiling and its exact per-root step",
    );
    assert_eq!(
        (over_one, over_two),
        (429_245, 429_350),
        "the gate-asserted totals for MAX_ROOTS + 1 and MAX_ROOTS + 2 roots reproduce \
         (measured per-root step {step}, ceiling total {at_ceiling})",
    );
    assert!(
        over_two < MAX_IMAGE_BYTES,
        "neither total is refused for its size: Roots is what refuses them",
    );
}

/// The number of declared fields each root of the wide corpus carries. It stays well
/// inside the record-field bound so that the corpus scales by *roots*, which is the axis
/// that reaches the image byte ceiling.
const WIDE_FIELDS: usize = 64;

/// The wide corpus at `roots` occurrences: one Product of [`WIDE_FIELDS`] int fields, and
/// `roots` keyless roots over it, each demanding its whole-payload site and every field
/// leaf. Its site count is `roots * (WIDE_FIELDS + 1)`.
fn wide_corpus(roots: usize) -> Result<Vec<u8>, ImageBuildError> {
    let mut draft_owner = ImageDraft::new();
    let mut draft = admitted(&mut draft_owner);
    let resource = draft.intern_string("R").expect("a within-domain mint");
    let field_names: Vec<_> = (0..WIDE_FIELDS)
        .map(|n| {
            draft
                .intern_string(&format!("f{n}"))
                .expect("a within-domain mint")
        })
        .collect();
    let record = draft
        .add_record_type(RecordTypeDef {
            name: resource,
            fields: field_names
                .iter()
                .map(|name| FieldDef {
                    name: *name,
                    ty: ImageType::scalar(Scalar::Int),
                    required: true,
                })
                .collect(),
        })
        .expect("a within-domain mint");
    draft.set_application_identity(LedgerIdBytes::from_bytes(APPLICATION_ID));
    let value = draft.value_scalar(Scalar::Int);
    draft
        .declare_product(
            &admitted_plan(),
            LedgerIdBytes::from_bytes(PRODUCT_ID),
            record,
            (0..WIDE_FIELDS)
                .map(|n| DeclarationMemberDef {
                    parent: None,
                    shape: DeclarationMemberShape::Field {
                        id: member_id(n),
                        required: true,
                        value,
                    },
                })
                .collect(),
        )
        .expect("a well-formed declaration");
    let members = draft
        .product_members(LedgerIdBytes::from_bytes(PRODUCT_ID))
        .expect("declared");
    for n in 0..roots {
        let name = draft
            .intern_string(&format!("r{n}"))
            .expect("a within-domain mint");
        let admitted = draft
            .add_root_occurrence(
                &admitted_plan(),
                LedgerIdBytes::from_bytes(PRODUCT_ID),
                RootOccurrenceDef {
                    name,
                    keys: Vec::new(),
                    placement: placement(n),
                    indexes: Vec::new().into(),
                },
            )
            .expect("the Product is declared");
        site(
            &mut draft,
            admitted.occurrence(),
            admitted.placement_path(),
            SemanticTarget::WholePayload,
        );
        for member in &members {
            site(
                &mut draft,
                admitted.occurrence(),
                member.path(),
                SemanticTarget::FieldLeaf,
            );
        }
    }
    draft.encode().map(|image| image.bytes)
}

/// Corpus 4: the highest whole-image-fitting site count `H` of the wide corpus family,
/// derived rather than guessed.
///
/// `MAX_SITES` is a table capacity, not a reachable width: every site carries wire cost, so
/// the image byte ceiling binds first. `H` is found by bisecting the corpus over its root
/// axis, and the pair `H` / the next step freezes which bound answers on each side — `H`
/// encodes and independently verifies with exactly `H` sealed sites, while one root more is
/// refused for its *size* and never for its site count. Without this the site-cap corpora
/// would be asserting behavior at a width no image can actually reach.
#[test]
fn the_highest_fitting_site_count_is_bounded_by_image_bytes_not_by_the_table() {
    let (mut fits, mut over) = (1usize, MAX_SITES / (WIDE_FIELDS + 1) + 1);
    assert!(wide_corpus(fits).is_ok(), "the smallest corpus encodes");
    assert!(
        wide_corpus(over).is_err(),
        "the corpus at the table cap does not"
    );
    while over - fits > 1 {
        let middle = fits + (over - fits) / 2;
        if wide_corpus(middle).is_ok() {
            fits = middle;
        } else {
            over = middle;
        }
    }

    let bytes = wide_corpus(fits).expect("H fits");
    let image = marrow_verify::verify(&bytes).expect("H verifies independently");
    let h = image.sites().len();
    assert_eq!(h, fits * (WIDE_FIELDS + 1), "every demand became a row");
    assert!(
        h < MAX_SITES,
        "the image byte ceiling binds before the site table: H = {h}",
    );
    assert_eq!(
        wide_corpus(over).expect_err("one root past H"),
        ImageBuildError::ImageTooLarge,
        "the next step is refused for its size, not for its site count",
    );
    assert!(bytes.len() <= MAX_IMAGE_BYTES);
    assert_eq!(
        (h, bytes.len()),
        (H_SITES, H_BYTES),
        "the highest fitting site corpus is frozen",
    );
}

/// The frozen highest-fitting site corpus: its site count and its exact framed size.
///
/// `H_` here abbreviates *highest-fitting* — a measured corpus figure, not the maximum-live
/// accounting's declared-ceiling term of the same prefix (`H_DURABLE_GRAPH_BYTES`,
/// `H_VERIFIED_DURABLE_GRAPH_BYTES`, `H_OWNED_BYTES`).
///
/// 7,150 sites is 1,042 short of `MAX_SITES`, so this corpus family cannot reach the site
/// table's last thousand rows at all. That is the same conclusion the 8,192/8,193 policy
/// pair reaches from the other side, measured here rather than argued.
const H_SITES: usize = 7_150;
const H_BYTES: usize = 523_779;
