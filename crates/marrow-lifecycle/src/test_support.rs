//! The lifecycle crate's shared corpus: one compiled single-field store and the fixture
//! helpers its in-crate suites present images against.
//!
//! One populated single-field store is enough for admission, recovery and apply: each suite
//! varies the *image* it presents, not the corpus. Keeping the fixture here means one
//! temporary directory owner and one identity ledger, so a suite cannot silently disagree
//! with another about what a populated store contains. The scratch directory and the
//! compile helpers are the same files the integration suites use.

use std::path::Path;

use marrow_verify::VerifiedImage;

use crate::{
    EngineKind, LogicalHead, ProvisionRequest, StoreEnvelope, StoreInstanceId, accepted_ceiling,
    active_binding, head_map, prepare,
};

#[path = "../tests/support/compile.rs"]
pub(crate) mod compile;
#[path = "../tests/support/scratch.rs"]
mod scratch;

pub(crate) use scratch::Scratch;

pub(crate) const SOURCE: &str = "resource Counter { required value: int }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): int { return ^counters[n].value ?? 0 }\n";
pub(crate) const IDS: &str = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\nid product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\nid field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\nid root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\nid key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\nhigh-water 0\nend\n";

/// The image bytes of `source` under the corpus ledger.
pub(crate) fn compile_bytes(source: &str) -> Vec<u8> {
    compile::compile_bytes(source, IDS)
}

pub(crate) fn request(image: &VerifiedImage, instance: StoreInstanceId) -> ProvisionRequest {
    ProvisionRequest {
        envelope: StoreEnvelope {
            instance,
            writer_toolchain: "0.1.0".into(),
            engine_kind: EngineKind::Redb,
            engine_format_version: marrow_kernel::durable::NATIVE_ENGINE_FORMAT_VERSION,
        },
        head: LogicalHead::provision(
            active_binding(image),
            accepted_ceiling(image),
            head_map(image).expect("head map"),
        ),
    }
}

pub(crate) fn populate_counter(dir: &Path, image: &VerifiedImage) {
    use marrow_kernel::codec::{key::KeyScalar, value::RuntimeScalar};
    use marrow_kernel::durable::{DemandCoverage, Durable, EntryValue, InvocationGrant};
    use marrow_kernel::equality::ValueDomain;

    let write_site = image
        .sites()
        .iter()
        .position(|site| {
            matches!(
                site,
                marrow_verify::SealedSite::Flat {
                    root: 0,
                    target: marrow_verify::SealedSiteTarget::WholePayload,
                }
            )
        })
        .expect("compiled entry write") as u16;
    let crate::AttachOutcome::AlreadyActive(mut attachment) =
        crate::attach(dir, prepare(image.clone())).expect("attach")
    else {
        panic!("initial binding");
    };
    let (_, host) = attachment.bridge();
    let mut txn = host
        .txn_session(
            InvocationGrant::full_store(),
            DemandCoverage {
                read: true,
                write: true,
            },
        )
        .expect("transaction");
    let site = txn.site(write_site);
    txn.create_entry(
        &site,
        &[KeyScalar::Int(7)],
        EntryValue {
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(42)))],
            groups: Vec::new(),
        },
    )
    .expect("write populated entry");
    assert!(matches!(
        txn.commit(),
        marrow_kernel::durable::CommitResult::Committed
    ));
}
