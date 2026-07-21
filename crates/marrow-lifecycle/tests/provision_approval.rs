//! The provision report and approval over a real compiled durable image: the report renders
//! in source vocabulary with no identity hash, provision refuses without a matching approval,
//! and an accepted provision round-trips through open.

use std::path::{Path, PathBuf};

use marrow_lifecycle::{
    ProvisionApproval, ProvisionImageError, ProvisionReport, StoreInstanceId, open, provision_image,
};
use marrow_verify::{VerifiedImage, verify};

const SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

fn compile() -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        SOURCE.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(IDS.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}

fn schemas(
    image: &VerifiedImage,
) -> (
    Vec<marrow_kernel::durable::StoreSchema>,
    Vec<marrow_kernel::durable::SiteSpec>,
) {
    marrow_vm::derive_store_schemas(image).expect("flat-executable")
}

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // A process-monotonic counter guarantees two parallel scratch dirs never collide even
        // when minted in the same nanosecond, so one test's Drop cleanup cannot remove
        // another's store mid-provision.
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "marrow-provision-approval-{}-{nonce}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Self { dir }
    }
    fn store(&self) -> PathBuf {
        self.dir.join("store")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The rendered report is in source vocabulary — the destination, the roots by name, the
/// effects and ceiling in demand terms — and carries no 32- or 64-character hex identity
/// string. This is the absence gate for the "never a raw hash a human would retype" rule.
#[test]
fn the_report_names_roots_in_source_vocabulary_with_no_identity_hash() {
    let image = compile();
    let (schemas, _sites) = schemas(&image);
    let dest = Path::new("/tmp/notes-store");
    let report = ProvisionReport::new(dest, &image, &schemas);
    let rendered = report.render();

    assert!(
        rendered.contains("counters"),
        "the root is named: {rendered}"
    );
    assert!(
        rendered.contains("/tmp/notes-store"),
        "the destination is named",
    );
    // No run of 32+ hex characters (a 16- or 32-byte identity spelled out).
    let mut run = 0usize;
    for ch in rendered.chars() {
        if ch.is_ascii_hexdigit() {
            run += 1;
            assert!(
                run < 32,
                "the report must not contain an identity hash: {rendered}"
            );
        } else {
            run = 0;
        }
    }
}

/// Provision refuses when the approval token does not match the report it would write — a
/// store is never provisioned without an auditable acceptance of the exact report.
#[test]
fn provision_refuses_without_a_matching_approval() {
    let image = compile();
    let (schemas_v, sites) = schemas(&image);
    let scratch = Scratch::new();

    let wrong = ProvisionApproval::from_token("not-the-right-token");
    let refused = provision_image(&scratch.store(), &image, schemas_v, sites, &wrong);
    assert!(
        matches!(refused, Err(ProvisionImageError::Unapproved)),
        "a mismatched approval is refused",
    );
    // Nothing was published.
    assert!(
        !scratch.store().exists(),
        "a refused provision writes no store"
    );
}

/// An accepted provision publishes the store and round-trips: open reads back the same store
/// instance and active binding the image derives.
#[test]
fn an_accepted_provision_round_trips_through_open() {
    let image = compile();
    let (schemas_v, sites) = schemas(&image);
    let scratch = Scratch::new();

    let report = ProvisionReport::new(&scratch.store(), &image, &schemas_v);
    let approval = ProvisionApproval::accept(&report);
    let provisioned = provision_image(
        &scratch.store(),
        &image,
        schemas_v.clone(),
        sites.clone(),
        &approval,
    )
    .expect("provision");

    let opened = open(&scratch.store(), schemas_v, sites).expect("open");
    assert_eq!(
        opened.envelope.instance, provisioned.instance,
        "the opened store carries the provisioned instance",
    );
    assert_eq!(
        opened.head.binding,
        marrow_lifecycle::active_binding(&image),
        "the head records the image's active binding",
    );
    // The instance is a well-formed 32-hex spelling.
    assert_eq!(
        provisioned.instance.to_hex().len(),
        32,
        "the instance renders as 32 hex characters",
    );
    let _ = StoreInstanceId::from_bytes(*provisioned.instance.bytes());
}
