use marrow_kernel::durable::{CommitRecovery, DurableCommitState};
use marrow_lifecycle::{
    ENGINE_FILE, OpenError, OpenStore, ProvisionApproval, ProvisionReport, open, provision_image,
};

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

const SOURCE: &str = r#"resource Counter {
    required value: int
}

store ^counters[id: int]: Counter

pub fn readValue(id: int): int? {
    return ^counters[id].value
}
"#;

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "marrow-commit-outcome-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn compile() -> (
    marrow_verify::VerifiedImage,
    Vec<marrow_kernel::durable::StoreSchema>,
    Vec<marrow_kernel::durable::SiteSpec>,
) {
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
    let image = marrow_verify::verify(&compiled.image.bytes).expect("verify");
    let (schemas, sites) = marrow_vm::derive_store_schemas(&image).expect("durable schema");
    (image, schemas, sites)
}

fn provision_fixture(
    store: &std::path::Path,
) -> (
    Vec<marrow_kernel::durable::StoreSchema>,
    Vec<marrow_kernel::durable::SiteSpec>,
) {
    let (image, schemas, sites) = compile();
    let report = ProvisionReport::new(store, &image, &schemas);
    let approval = ProvisionApproval::accept(&report);
    provision_image(store, &image, schemas.clone(), sites.clone(), &approval)
        .expect("provision fixture");
    (schemas, sites)
}

#[test]
fn recovery_consumes_the_old_owner_and_returns_only_a_known_reopened_owner() {
    let _signature: fn(OpenStore, CommitRecovery) -> (DurableCommitState, Option<OpenStore>) =
        OpenStore::resolve_recovery;
}

#[test]
fn ordinary_open_does_not_recreate_a_missing_engine_file() {
    let scratch = Scratch::new();
    let store = scratch.0.join("store");
    let (schemas, sites) = provision_fixture(&store);
    let engine = store.join(ENGINE_FILE);
    std::fs::remove_file(&engine).expect("remove provisioned engine");

    assert!(
        matches!(open(&store, schemas, sites), Err(OpenError::Incomplete)),
        "a store missing its engine artifact is incomplete, never freshly created",
    );
    assert!(
        !engine.exists(),
        "ordinary open must leave a missing engine path absent",
    );
}

#[test]
fn ordinary_open_does_not_adopt_empty_or_malformed_engine_files() {
    for (label, bytes) in [("empty", b"".as_slice()), ("malformed", b"not redb")] {
        let scratch = Scratch::new();
        let store = scratch.0.join(label);
        let (schemas, sites) = provision_fixture(&store);
        let engine = store.join(ENGINE_FILE);
        std::fs::write(&engine, bytes).expect("replace engine with invalid body");

        assert!(
            open(&store, schemas, sites).is_err(),
            "{label} engine body must be refused",
        );
        assert_eq!(
            std::fs::read(&engine).expect("read refused engine body"),
            bytes,
            "ordinary open must not stamp or adopt a {label} engine body",
        );
    }
}
