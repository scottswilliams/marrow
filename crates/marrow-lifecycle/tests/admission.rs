//! Image admission against a store: the effect ceiling, the four-way authority
//! intersection, the provision report and its approval, shared-product provisioning, and
//! the image/store attachment itself.

mod support;

#[path = "admission/attachment.rs"]
mod attachment;
#[path = "admission/authority_intersection.rs"]
mod authority_intersection;
#[path = "admission/ceiling.rs"]
mod ceiling;
#[path = "admission/provision_approval.rs"]
mod provision_approval;
#[path = "admission/shared_product.rs"]
mod shared_product;
