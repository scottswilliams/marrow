//! The one store-publication path the lifecycle suites share.
//!
//! Two publication mechanisms exist and they test different things. The approval path
//! ([`try_provision_approved`]) is what a tool calls: it renders the report, accepts it, and
//! publishes. The request path ([`provision_from`], [`provision_with_head`]) hands
//! `provision` a head the caller built, which is how the admission matrices forge one
//! binding, ceiling, or pin fact at a time. Both live here so a suite cannot quietly grow a
//! third.

use std::path::Path;

use marrow_lifecycle::{
    AttachOutcome, EngineKind, LifecycleError, LogicalHead, ProvisionApproval, ProvisionImageError,
    ProvisionReport, ProvisionRequest, Provisioned, StoreEnvelope, StoreInstanceId,
    accepted_ceiling, active_binding, attach, head_map, prepare, provision, provision_image,
};
use marrow_verify::VerifiedImage;

/// A fresh envelope for a store this suite is about to publish.
pub fn envelope(instance: StoreInstanceId) -> StoreEnvelope {
    StoreEnvelope {
        instance,
        writer_toolchain: "0.1.0".to_string(),
        engine_kind: EngineKind::Redb,
        engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
    }
}

/// The head a first provision under `image` records.
pub fn head_of(image: &VerifiedImage) -> LogicalHead {
    LogicalHead::provision(
        active_binding(image),
        accepted_ceiling(image),
        head_map(image).expect("head map"),
    )
}

/// Publish a store at `dir` under `image`, returning the instance it was minted with.
pub fn provision_from(dir: &Path, image: &VerifiedImage) -> StoreInstanceId {
    let instance = StoreInstanceId::draw().expect("entropy");
    provision(
        dir,
        ProvisionRequest {
            envelope: envelope(instance),
            head: head_of(image),
        },
    )
    .expect("provision");
    instance
}

/// Publish a store at `dir` under a caller-built head, for the cases that forge one
/// admission fact at a time.
pub fn provision_with_head(dir: &Path, head: LogicalHead) {
    provision(
        dir,
        ProvisionRequest {
            envelope: envelope(StoreInstanceId::draw().expect("entropy")),
            head,
        },
    )
    .expect("provision");
}

/// The tool's path: render the report, accept exactly it, publish. The approval is accepted
/// from the report this call itself renders, so nothing but the image decides the result.
pub fn try_provision_approved(
    dir: &Path,
    image: &VerifiedImage,
) -> Result<Provisioned, ProvisionImageError> {
    let prepared = prepare(image.clone());
    let report = ProvisionReport::new(dir, &prepared)?;
    let approval = ProvisionApproval::accept(&report);
    provision_image(dir, &prepared, &approval)
}

/// [`try_provision_approved`] where the publication is expected to succeed.
pub fn provision_approved(dir: &Path, image: &VerifiedImage) {
    try_provision_approved(dir, image).expect("provision");
}

pub fn attach_image(dir: &Path, image: &VerifiedImage) -> Result<AttachOutcome, LifecycleError> {
    attach(dir, prepare(image.clone()))
}
