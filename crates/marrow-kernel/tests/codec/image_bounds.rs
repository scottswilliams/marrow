//! Drift gates for the durable bounds the kernel mirrors from the image.
//!
//! `marrow-kernel` keeps **no production dependency** on `marrow-image`: the kernel
//! must bound its own recursion without an image in hand, so the value-shape depth
//! cap is spelled in `codec::value` and the member-tree depth cap in
//! `durable::schema`. Each spelling was a hand-copy — two constants stating one
//! contract with nothing tying them together, so either could move and leave the
//! other silently behind. The image is a **dev** dependency, which is enough to
//! close the copies here without adding an edge to the shipped crate.
//!
//! Each coupling is a `const` assertion, so a divergence fails the build rather than
//! a run: there is no way to land a moved bound and read a green suite.

use marrow_image::bounds::{
    MAX_DURABLE_DEPTH as IMAGE_MEMBER_DEPTH, MAX_DURABLE_VALUE_DEPTH as IMAGE_VALUE_DEPTH,
};
use marrow_kernel::codec::value::MAX_DURABLE_VALUE_DEPTH as KERNEL_VALUE_DEPTH;
use marrow_kernel::durable::MAX_DURABLE_DEPTH as KERNEL_MEMBER_DEPTH;

/// The hand-copy, closed at compile time. Moving either constant without the other
/// fails this crate's test build.
const _: () = assert!(
    KERNEL_VALUE_DEPTH == IMAGE_VALUE_DEPTH,
    "the kernel's value-shape depth cap must equal the image bound it mirrors",
);

/// The member-tree hand-copy, closed the same way. The kernel's store-schema builder
/// bounds branch and group nesting with its own constant; the image's decoder bounds
/// the member rows that projection is derived from. One contract, so one value.
const _: () = assert!(
    KERNEL_MEMBER_DEPTH == IMAGE_MEMBER_DEPTH,
    "the kernel's durable member-depth cap must equal the image bound it mirrors",
);

/// The same fact as a named test, so the gate appears in the suite it protects and a
/// reader of the failure list is told which two owners disagreed.
#[test]
fn the_kernel_value_depth_cap_mirrors_the_image_bound() {
    assert_eq!(
        KERNEL_VALUE_DEPTH, IMAGE_VALUE_DEPTH,
        "marrow_kernel::codec::value::MAX_DURABLE_VALUE_DEPTH and \
         marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH state one contract",
    );
}

/// The member-depth fact as a named test, for the same reason.
#[test]
fn the_kernel_member_depth_cap_mirrors_the_image_bound() {
    assert_eq!(
        KERNEL_MEMBER_DEPTH, IMAGE_MEMBER_DEPTH,
        "marrow_kernel::durable::MAX_DURABLE_DEPTH and \
         marrow_image::bounds::MAX_DURABLE_DEPTH state one contract",
    );
}

/// The member bound is only meaningful if both sides count the same thing. Both owners
/// document a top-level member as depth 1 with a member of a group or branch one deeper.
/// This case pins that unit on the kernel's side at the boundary: a field nested under
/// `MAX_DURABLE_DEPTH - 1` branches sits at exactly the bound and mints, and one branch
/// further is refused — the same N/N+1 the image states over its member rows.
#[test]
fn the_shared_member_bound_counts_a_top_level_member_as_depth_one() {
    use marrow_kernel::codec::value::ScalarKind;
    use marrow_kernel::durable::{SchemaBuildError, StoreSchemaBuilder};

    let nest = |branches: usize| {
        let mut builder = StoreSchemaBuilder::root("root", vec![ScalarKind::Int]);
        for _ in 0..branches {
            builder.open_branch("b", vec![ScalarKind::Int]);
        }
        builder.scalar_field("leaf", ScalarKind::Int, false);
        for _ in 0..branches {
            builder.close_branch();
        }
        builder.finish()
    };

    assert!(
        nest(KERNEL_MEMBER_DEPTH - 1).is_ok(),
        "a member at exactly the shared bound mints",
    );
    assert_eq!(
        nest(KERNEL_MEMBER_DEPTH),
        Err(SchemaBuildError::TooDeep),
        "one member past the shared bound is refused",
    );
}

/// The value bound is only meaningful if both sides count the same thing. Both owners
/// document a top-level field value as depth 1 with each nested composite one
/// deeper, and the kernel's encoder and decoder are held to that by their own N/N+1
/// cases in `codec::value`. This case pins the shared unit at the boundary: the
/// depth is a count of composites, and scalar leaves are free.
#[test]
fn the_shared_bound_counts_composites_not_leaves() {
    use marrow_kernel::codec::value::{
        RuntimeScalar, ScalarKind, ValueShapeBuilder, decode_domain, encode_domain,
    };
    use marrow_kernel::equality::ValueDomain;

    // The value and the shape are separate owners: a durable value is caller-built, while
    // a shape is minted only by the command builder, which refuses one composite past the
    // bound. So each half nests on its own.
    let nest_value = |composites: usize| {
        let mut value = ValueDomain::Scalar(RuntimeScalar::Int(7));
        for _ in 0..composites {
            value = ValueDomain::Product {
                ty: 3,
                fields: vec![Some(value)],
            };
        }
        value
    };
    let nest_shape = |composites: usize| {
        let mut builder = ValueShapeBuilder::new();
        for _ in 0..composites {
            builder.open_product(3);
        }
        builder.scalar(ScalarKind::Int);
        for _ in 0..composites {
            builder.close();
        }
        builder.finish()
    };

    let deepest = nest_value(KERNEL_VALUE_DEPTH);
    let deepest_shape = nest_shape(KERNEL_VALUE_DEPTH).expect("the bound's own depth mints");
    let bytes = encode_domain(&deepest).expect("the bound's own depth is admitted");
    assert_eq!(
        decode_domain(&bytes, &deepest_shape).as_ref(),
        Some(&deepest)
    );

    let over = nest_value(KERNEL_VALUE_DEPTH + 1);
    assert!(
        encode_domain(&over).is_err(),
        "one composite past the shared bound is refused",
    );
}
