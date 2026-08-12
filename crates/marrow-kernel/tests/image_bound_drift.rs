//! Drift gate for the durable bounds the kernel codec mirrors from the image.
//!
//! `marrow-kernel` keeps **no production dependency** on `marrow-image`: the kernel
//! must bound its own recursion without an image in hand, so the value-shape depth
//! cap is spelled in `codec::value`. That spelling was a hand-copy — two constants
//! stating one contract with nothing tying them together, so either could move and
//! leave the other silently behind. The image is a **dev** dependency, which is
//! enough to close the copy here without adding an edge to the shipped crate.
//!
//! The coupling is a `const` assertion, so a divergence fails the build rather than
//! a run: there is no way to land a moved bound and read a green suite.

use marrow_image::bounds::MAX_DURABLE_VALUE_DEPTH as IMAGE_VALUE_DEPTH;
use marrow_kernel::codec::value::MAX_DURABLE_VALUE_DEPTH as KERNEL_VALUE_DEPTH;

/// The hand-copy, closed at compile time. Moving either constant without the other
/// fails this crate's test build.
const _: () = assert!(
    KERNEL_VALUE_DEPTH == IMAGE_VALUE_DEPTH,
    "the kernel's value-shape depth cap must equal the image bound it mirrors",
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

/// The bound is only meaningful if both sides count the same thing. Both owners
/// document a top-level field value as depth 1 with each nested composite one
/// deeper, and the kernel's encoder and decoder are held to that by their own N/N+1
/// cases in `codec::value`. This case pins the shared unit at the boundary: the
/// depth is a count of composites, and scalar leaves are free.
#[test]
fn the_shared_bound_counts_composites_not_leaves() {
    use marrow_kernel::codec::value::{
        RuntimeScalar, ScalarKind, ValueShape, decode_domain, encode_domain,
    };
    use marrow_kernel::equality::ValueDomain;

    let nest = |composites: usize| {
        let mut value = ValueDomain::Scalar(RuntimeScalar::Int(7));
        let mut shape = ValueShape::Scalar(ScalarKind::Int);
        for _ in 0..composites {
            value = ValueDomain::Product {
                ty: 3,
                fields: vec![Some(value)],
            };
            shape = ValueShape::Product {
                ty: 3,
                fields: vec![shape],
            };
        }
        (value, shape)
    };

    let (deepest, deepest_shape) = nest(KERNEL_VALUE_DEPTH);
    let bytes = encode_domain(&deepest).expect("the bound's own depth is admitted");
    assert_eq!(
        decode_domain(&bytes, &deepest_shape).as_ref(),
        Some(&deepest)
    );

    let (over, _) = nest(KERNEL_VALUE_DEPTH + 1);
    assert!(
        encode_domain(&over).is_err(),
        "one composite past the shared bound is refused",
    );
}
