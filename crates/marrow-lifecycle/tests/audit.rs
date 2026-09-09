//! The read-only store audit end to end over a real provisioned store: the exact-binding
//! gate, engine-byte preservation, the kernel's walk rendered in source vocabulary, and
//! the content digest's stability and physical-integrity limitation.

use std::path::{Path, PathBuf};

use marrow_kernel::codec::key::KeyScalar;
use marrow_kernel::codec::value::RuntimeScalar;
use marrow_kernel::durable::{DemandCoverage, Durable, EntryValue, InvocationGrant};
use marrow_kernel::equality::ValueDomain;
use marrow_lifecycle::{
    AttachOutcome, AuditError, ChangedFact, ENGINE_FILE, EngineKind, LogicalHead, ProvisionRequest,
    StoreEnvelope, StoreInstanceId, active_binding, attach, audit, head_map, prepare, provision,
};
use marrow_verify::{VerifiedImage, verify};

/// `^people[id: int]` with a required `name`, a sparse `email`, and a unique index on
/// `email`, plus one mutating export so the accepted ceiling admits writes.
const SOURCE: &str = r#"resource Person {
    required name: string
    email: string
}

store ^people[id: int]: Person {
    index byEmail[email] unique
}

pub fn add(id: int, name: string) {
    transaction {
        ^people[id] = Person(name: name)
    }
}

pub fn nameOf(id: int): string? {
    return ^people[id].name
}
"#;

const IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Person 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Person.name 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Person.email 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root people 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key people.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index people.byEmail 1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c\n\
     high-water 0\n\
     end\n";

/// [`IDS`] with a different index identity: the same program under another ledger, whose
/// index cells live in a different family.
const OTHER_INDEX_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Person 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Person.name 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Person.email 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root people 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key people.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id index people.byEmail 2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c\n\
     high-water 0\n\
     end\n";

/// A code-only edit of [`SOURCE`]: the same durable contract and export set.
const EDITED_SOURCE: &str = r#"resource Person {
    required name: string
    email: string
}

store ^people[id: int]: Person {
    index byEmail[email] unique
}

pub fn add(id: int, name: string) {
    transaction {
        ^people[id] = Person(name: name, email: "x")
    }
}

pub fn nameOf(id: int): string? {
    return ^people[id].name
}
"#;

/// [`SOURCE`] with one more export: an interface change.
const WIDENED_SOURCE: &str = r#"resource Person {
    required name: string
    email: string
}

store ^people[id: int]: Person {
    index byEmail[email] unique
}

pub fn add(id: int, name: string) {
    transaction {
        ^people[id] = Person(name: name)
    }
}

pub fn nameOf(id: int): string? {
    return ^people[id].name
}

pub fn ping(): int {
    return 1
}
"#;

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
    base: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "marrow-audit-{tag}-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ));
        std::fs::create_dir(&base).expect("unique scratch base");
        Self { base }
    }

    fn store(&self, name: &str) -> PathBuf {
        self.base.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn provision_from(dir: &Path, image: &VerifiedImage) {
    let envelope = StoreEnvelope {
        instance: StoreInstanceId::draw().expect("entropy"),
        writer_toolchain: "0.1.0".to_string(),
        engine_kind: EngineKind::Redb,
        engine_format_version: 1,
    };
    let head = LogicalHead::provision(
        active_binding(image),
        marrow_lifecycle::accepted_ceiling(image),
        head_map(image).expect("head map"),
    );
    provision(dir, ProvisionRequest { envelope, head }).expect("provision");
}

/// Create `people[id]` with `name` and, when given, `email`, through the kernel on the
/// active image's own attachment.
fn add_person(dir: &Path, image: &VerifiedImage, id: i64, name: &str, email: Option<&str>) {
    let AttachOutcome::AlreadyActive(mut attachment) =
        attach(dir, prepare(image.clone())).expect("attach")
    else {
        panic!("the provisioned image is already active");
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
        .expect("txn");
    let site = image
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
        .expect("the program writes a whole entry") as u16;
    let site = txn.site(site);
    txn.create_entry(
        &site,
        &[KeyScalar::Int(id)],
        EntryValue {
            fields: vec![
                Some(ValueDomain::Scalar(RuntimeScalar::Str(name.into()))),
                email.map(|email| ValueDomain::Scalar(RuntimeScalar::Str(email.into()))),
            ],
            groups: Vec::new(),
        },
    )
    .expect("create");
    assert!(matches!(
        txn.commit(),
        marrow_kernel::durable::CommitResult::Committed
    ));
}

fn walked(dir: &Path, image: &VerifiedImage) -> (marrow_lifecycle::StoreAudit, Vec<String>) {
    let audit = audit(dir, prepare(image.clone())).expect("the audit runs");
    let findings = audit
        .findings
        .iter()
        .map(|finding| format!("{} at {}", finding.code.as_str(), finding.place))
        .collect();
    (audit, findings)
}

fn digest_of(audit: &marrow_lifecycle::StoreAudit) -> String {
    audit.digest.to_hex()
}

#[test]
fn logical_inspection_preserves_engine_head_and_envelope_bytes() {
    for (tag, replacement) in [
        ("clean", None),
        ("scalar", Some(b'b')),
        ("invalid", Some(0xff)),
    ] {
        let scratch = Scratch::new(tag);
        let store = scratch.store("store");
        let image = compile(SOURCE, IDS);
        provision_from(&store, &image);
        add_person(&store, &image, 1, "Ada Lovelace", None);
        if let Some(replacement) = replacement {
            let engine = store.join(ENGINE_FILE);
            let mut bytes = std::fs::read(&engine).expect("engine");
            let at = bytes
                .windows(12)
                .position(|v| v == b"Ada Lovelace")
                .expect("name");
            bytes[at + 2] = replacement;
            std::fs::write(engine, bytes).expect("change scalar bytes");
        }
        let before: Vec<_> = [
            ENGINE_FILE,
            marrow_lifecycle::HEAD_FILE,
            marrow_lifecycle::ENVELOPE_FILE,
        ]
        .into_iter()
        .map(|name| (name, std::fs::read(store.join(name)).expect("before")))
        .collect();
        let outcome = audit(&store, prepare(image)).expect("logical inspection");
        assert_eq!(outcome.is_clean(), replacement != Some(0xff));
        for (name, bytes) in before {
            assert!(
                bytes == std::fs::read(store.join(name)).expect("after"),
                "{tag}: audit changed {name}"
            );
        }
    }
}

#[test]
fn a_populated_store_audits_clean_with_a_stable_digest_that_tracks_writes() {
    let scratch = Scratch::new("clean");
    let store = scratch.store("store");
    let image = compile(SOURCE, IDS);
    provision_from(&store, &image);
    add_person(&store, &image, 1, "Ada", Some("ada@example.org"));
    add_person(&store, &image, 2, "Grace", None);

    let (first, findings) = walked(&store, &image);
    assert!(findings.is_empty(), "{findings:?}");
    assert!(first.is_clean());
    assert_eq!(first.image_id, image.image_id());
    let (second, _) = walked(&store, &image);
    assert_eq!(digest_of(&first), digest_of(&second));
    {
        let summary = &second.summary;
        assert_eq!(summary.entries, 2);
        assert_eq!(summary.index_cells, 1);
    }

    add_person(&store, &image, 3, "Linus", None);
    let (third, findings) = walked(&store, &image);
    assert!(findings.is_empty(), "{findings:?}");
    assert_ne!(digest_of(&first), digest_of(&third));

    // The audit released the lock: the store attaches again.
    assert!(attach(&store, prepare(image.clone())).is_ok());
}

/// The same program under another ledger numbers its cells identically but keys its index
/// cells by a different identity, so an engine swapped under the other provision's head
/// audits with one index's cells outside the schema and the other's cells missing.
#[test]
fn an_engine_swapped_under_another_provisions_head_is_reported() {
    let scratch = Scratch::new("swap");
    let populated = scratch.store("populated");
    let other = scratch.store("other");
    let image = compile(SOURCE, IDS);
    let other_image = compile(SOURCE, OTHER_INDEX_IDS);
    provision_from(&populated, &image);
    add_person(&populated, &image, 1, "Ada", Some("ada@example.org"));
    provision_from(&other, &other_image);

    let (_, findings) = walked(&other, &other_image);
    assert!(findings.is_empty(), "a fresh store is clean: {findings:?}");

    std::fs::copy(populated.join(ENGINE_FILE), other.join(ENGINE_FILE)).expect("swap engine");
    let (swapped, findings) = walked(&other, &other_image);
    assert!(!swapped.is_clean());
    assert_eq!(findings.len(), 2, "{findings:?}");
    assert_eq!(
        findings[0],
        "store.audit_index_missing at ^people.index(2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c)[\"ada@example.org\"]"
    );
    assert_eq!(
        findings[1],
        "store.audit_outside_schema at ^people.index(1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c)"
    );
    // The digest is over the logical entries, which the swap carried across unchanged.
    let (source, _) = walked(&populated, &image);
    assert_eq!(digest_of(&source), digest_of(&swapped));
}

#[test]
fn a_same_shape_scalar_change_is_not_physical_integrity_evidence() {
    let scratch = Scratch::new("flip");
    let store = scratch.store("store");
    let image = compile(SOURCE, IDS);
    provision_from(&store, &image);
    add_person(&store, &image, 1, "Ada Lovelace", None);
    let before = audit(&store, prepare(image.clone())).expect("initial logical audit");

    let engine = store.join(ENGINE_FILE);
    let mut bytes = std::fs::read(&engine).expect("read engine");
    let at = bytes
        .windows(12)
        .position(|window| window == b"Ada Lovelace")
        .expect("the stored name is in the engine file");
    bytes[at + 2] = b'b';
    std::fs::write(&engine, bytes).expect("write engine");

    let audit = audit(&store, prepare(image.clone())).expect("the audit reports");
    assert!(
        audit.is_clean(),
        "the changed scalar still has its declared type"
    );
    assert_ne!(
        audit.digest, before.digest,
        "a prior content digest detects the change"
    );
}

#[test]
fn only_the_exact_active_binding_may_audit() {
    let scratch = Scratch::new("binding");
    let store = scratch.store("store");
    let image = compile(SOURCE, IDS);
    provision_from(&store, &image);

    let edited = compile(EDITED_SOURCE, IDS);
    assert!(matches!(
        audit(&store, prepare(edited)),
        Err(AuditError::ImageNotActive)
    ));
    let widened = compile(WIDENED_SOURCE, IDS);
    match audit(&store, prepare(widened)) {
        Err(AuditError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::Interface);
        }
        other => panic!("expected a contract-changed refusal, got {other:?}"),
    }
    // Neither refusal rebound the store: the original image is still active.
    assert!(matches!(
        attach(&store, prepare(image.clone())),
        Ok(AttachOutcome::AlreadyActive(_))
    ));
}

#[test]
fn a_held_store_and_an_absent_store_are_open_refusals() {
    let scratch = Scratch::new("open");
    let store = scratch.store("store");
    let image = compile(SOURCE, IDS);
    assert!(matches!(
        audit(&store, prepare(image.clone())),
        Err(AuditError::Open(
            marrow_lifecycle::OpenError::NotProvisioned
        ))
    ));
    provision_from(&store, &image);
    let holder = attach(&store, prepare(image.clone())).expect("attach");
    assert!(matches!(
        audit(&store, prepare(image.clone())),
        Err(AuditError::Open(marrow_lifecycle::OpenError::Lock(_)))
    ));
    drop(holder);
    assert!(audit(&store, prepare(image)).is_ok());
}

/// Version refusal precedes engine open, and logical inspection cannot convert an
/// older layout even when its active image is exact.
#[test]
fn unsupported_generations_refuses_audit_before_engine_open() {
    let image = compile(SOURCE, IDS);
    use marrow_lifecycle::FormatError;
    for (offset, version, expected) in [
        (4, 1, FormatError::UnknownVersion { found: 1 }),
        (5, 0, FormatError::UnsupportedImageVersion { found: 0 }),
        (5, 255, FormatError::UnsupportedImageVersion { found: 255 }),
    ] {
        for broken_engine in [true, false] {
            let scratch = Scratch::new("old-generation");
            let dir = scratch.store("store");
            provision_from(&dir, &image);
            let head_path = dir.join(marrow_lifecycle::HEAD_FILE);
            let current_head = std::fs::read(&head_path).expect("current head");
            let mut old_head = current_head.clone();
            old_head[offset] = version;
            let body_len = old_head.len() - 32;
            let digest = marrow_image::StoreHeadDigest::compute(&old_head[..body_len]);
            old_head[body_len..].copy_from_slice(digest.bytes());
            std::fs::write(&head_path, old_head).expect("generation-one head");
            if broken_engine {
                std::fs::write(dir.join(marrow_lifecycle::ENGINE_FILE), b"not an engine")
                    .expect("broken engine control");
            }
            let before: Vec<_> = [
                marrow_lifecycle::ENGINE_FILE,
                marrow_lifecycle::HEAD_FILE,
                marrow_lifecycle::ENVELOPE_FILE,
            ]
            .into_iter()
            .map(|name| (name, std::fs::read(dir.join(name)).expect("before refusal")))
            .collect();
            let outcome = audit(&dir, prepare(image.clone()));
            let error = outcome.expect_err("an older layout must not be opened");
            assert_eq!(error.code(), "store.format_version");
            assert!(matches!(
                error,
                AuditError::Open(marrow_lifecycle::OpenError::Admission(
                    marrow_lifecycle::AdmissionError {
                        entry: marrow_lifecycle::StoreEntry::Head,
                        fault: marrow_lifecycle::AdmissionFault::Format(
                            ref refusal
                        ),
                    }
                )) if *refusal == expected
            ));
            for (name, bytes) in before {
                assert_eq!(
                    std::fs::read(dir.join(name)).expect("after refusal"),
                    bytes,
                    "{name}"
                );
            }
            if !broken_engine {
                std::fs::write(head_path, current_head).expect("restore current head");
                assert!(
                    audit(&dir, prepare(image.clone()))
                        .expect("lock released")
                        .is_clean()
                );
            }
        }
    }
}
