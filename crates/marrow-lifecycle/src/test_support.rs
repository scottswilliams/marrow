//! Compiled fixtures and scratch stores shared by the lifecycle crate's in-crate suites.
//!
//! One populated single-field store is enough for admission, recovery and apply: each suite
//! varies the *image* it presents, not the corpus. Keeping the fixture here means one
//! temporary directory owner and one identity ledger, so a suite cannot silently disagree
//! with another about what a populated store contains.

use std::path::{Path, PathBuf};

use marrow_verify::VerifiedImage;

use crate::{
    EngineKind, LogicalHead, ProvisionRequest, StoreEnvelope, StoreInstanceId, accepted_ceiling,
    active_binding, head_map, prepare,
};

pub(crate) const SOURCE: &str = "resource Counter { required value: int }\nstore ^counters[id: int]: Counter\npub fn readValue(n: int): int { return ^counters[n].value ?? 0 }\n";
pub(crate) const IDS: &str = "marrow ids v0\nmachine-written by marrow; do not edit\nid application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\nid product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\nid field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\nid root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\nid key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\nhigh-water 0\nend\n";

pub(crate) fn compile_bytes(source: &str) -> Vec<u8> {
    compile_with_ids(source, IDS)
}

pub(crate) fn compile_with_ids(source: &str, ids: &str) -> Vec<u8> {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let project = marrow_project::capture(
        &manifest,
        vec![marrow_project::CapturedFile::new(
            "src/main.mw".into(),
            source.as_bytes().to_vec(),
        )],
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    compiled.image.bytes
}

pub(crate) struct Scratch(PathBuf);
impl Scratch {
    pub(crate) fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "marrow-lifecycle-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&path).expect("scratch");
        Self(path)
    }
    /// The scratch root, for a fixture that needs a sibling of the store directory.
    pub(crate) fn root(&self) -> &Path {
        &self.0
    }

    pub(crate) fn store(&self) -> PathBuf {
        self.0.join("store")
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("failed lifecycle fixture retained at {}", self.0.display());
            return;
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
