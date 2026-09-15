//! The draft fixtures the verifier's own unit tests build images with.
//!
//! They are `marrow-image`'s: the admitted-plan helper, the byte-poke forger, and the
//! site seam. One copy serves every crate that builds a hostile or well-formed image,
//! so this module names them where they live rather than restating them. The `#[path]`
//! reach is the workspace's shared-fixture idiom pending a test-support dev-dependency.

#[path = "../../../marrow-image/tests/common/admitted_plan.rs"]
pub(super) mod admitted_plan;
#[path = "../../../marrow-image/tests/common/image_forgery.rs"]
pub(super) mod image_forgery;
#[path = "../../../marrow-image/tests/common/site_seam.rs"]
pub(super) mod site_seam;
