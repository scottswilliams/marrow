//! The preserved native-provision refusal for an image whose roots share one durable
//! Product.
//!
//! A Product is a declaration and a root is an occurrence, so two roots over one resource
//! carry the same member ledger ids under different roots. The head identity map is a
//! bijection over the ledger id of every durable node across every root, so it refuses
//! that image — the numbering it would need is the physical layout owner's, not this
//! crate's.
//!
//! That refusal is deliberate and load-bearing: it is the named hold that keeps native
//! provisioning honest until the physical layout row derives complete root-prefixed
//! addresses. Until then a shared-Product program compiles, verifies, and executes
//! ephemerally, and only *persistent* provision is refused. This fixture pins that exact
//! typed outcome so the hold cannot be released by accident, and pins the single-root
//! control beside it so the refusal is proven specific to the sharing.

use std::path::{Path, PathBuf};

use marrow_lifecycle::{ProvisionApproval, ProvisionImageError, ProvisionReport, provision_image};
use marrow_verify::{VerifiedImage, verify};

const SHARED: &str = r#"resource Counter {
    required value: int
}

store ^a[id: int]: Counter
store ^b[id: int]: Counter

pub fn readA(n: int): int {
    return ^a[n].value ?? 0
}
"#;

const SHARED_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

const DISTINCT: &str = r#"resource Counter {
    required value: int
}

resource Tally {
    required total: int
}

store ^a[id: int]: Counter
store ^b[id: int]: Tally

pub fn readA(n: int): int {
    return ^a[n].value ?? 0
}
"#;

const DISTINCT_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id product Tally 1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Tally.total 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id root a 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id root b 1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b\n\
     id key a.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id key b.id 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

fn compile(source: &str, ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
    )];
    let project = marrow_project::capture(
        &manifest,
        files,
        Some(ids.as_bytes()),
        &marrow_project::CaptureLimits::DEFAULT,
    )
    .expect("capture");
    let compiled = marrow_compile::compile(&project).expect("compile");
    verify(&compiled.image.bytes).expect("verify")
}

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "marrow-shared-product-{tag}-{}-{nonce}-{counter}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).expect("create scratch");
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

/// Provision `image` into a fresh destination with a matching approval, returning the
/// outcome. The approval is accepted from the report the same call rebuilds, so nothing
/// but the image itself decides the result.
fn provision(dest: &Path, image: &VerifiedImage) -> Result<(), ProvisionImageError> {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    let report = ProvisionReport::new(dest, image, &schemas);
    let approval = ProvisionApproval::accept(&report);
    provision_image(dest, image, schemas, sites, &approval).map(|_| ())
}

#[test]
fn two_roots_over_one_product_are_refused_by_the_head_identity_map() {
    let image = compile(SHARED, SHARED_IDS);
    let scratch = Scratch::new("shared");
    let refused = provision(&scratch.store(), &image)
        .expect_err("a shared-Product image must not provision natively");
    assert!(
        matches!(refused, ProvisionImageError::Head(_)),
        "expected the head identity map to refuse the shared Product, got {refused:?}"
    );
    assert!(
        !scratch.store().exists(),
        "a refused provision must publish no store"
    );
}

#[test]
fn two_roots_over_distinct_products_still_provision() {
    // The control: the same two-root program with a Product each. Its durable node ids
    // are all distinct, so the head map is a bijection and provision completes — the
    // refusal above is caused by the sharing, not by having two roots.
    let image = compile(DISTINCT, DISTINCT_IDS);
    let scratch = Scratch::new("distinct");
    provision(&scratch.store(), &image).expect("two roots over distinct Products provision");
}
