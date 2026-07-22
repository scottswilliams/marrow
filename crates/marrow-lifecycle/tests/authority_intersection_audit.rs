//! The four-way authority-intersection audit (G03).
//!
//! Effective durable authority is `demand ∩ ceiling ∩ grant ∩ principal`, resolved before the
//! first engine call. This audit proves the order holds where all four terms meet, each
//! exercised through its real owner:
//!
//! 1. **demand ∩ ceiling** — an image whose verified demand exceeds the store's accepted
//!    deployment ceiling is refused at attach, before any engine call (`authority::admit`).
//! 2. **∩ grant (attenuation)** — over a store the ceiling admits, a read-only invocation grant
//!    denies a mutating demand at session open (`resolve_authority`), even though the ceiling
//!    would permit it.
//! 3. **∩ principal** — the reserved fourth term ([`PrincipalPredicate`]) is ⊤ today: it narrows
//!    the effective authority to itself (`X ∩ ⊤ = X`), participating in the order without adding
//!    or removing an atom. It is a reserved slot, not a compatibility promise; a future narrowing
//!    variant can only clear authority, never widen it.
//! 4. **all admit** — under a covering ceiling, a full grant, and the ⊤ principal, a read
//!    session opens and observes the store.
//!
//! Terms 1 (ceiling, at attach) and 2/4 (grant, at the kernel session) are distinct enforcement
//! points in the fixed order — the ceiling is resolved at attach with zero engine calls, the
//! grant at session open — so a broadened image never reaches the grant check, and an
//! over-attenuated grant never reaches the engine.

use std::path::{Path, PathBuf};

use marrow_kernel::durable::{DemandCoverage, InvocationGrant, PrincipalPredicate, SessionError};
use marrow_lifecycle::{
    AttachOutcome, LifecycleError, ProvisionApproval, ProvisionReport, attach, provision_image,
};
use marrow_verify::{VerifiedImage, verify};

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

const SHAPE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter
"#;

/// A read-only export: its demand union is the accepted ceiling of a store provisioned under it.
fn source_read_only() -> String {
    format!("{SHAPE}\npub fn readValue(n: int): int {{\n    return ^counters[n].value ?? 0\n}}\n")
}

/// The same export, same signature, broadened to also mutate `^counters.label`.
fn source_broadened() -> String {
    format!(
        "{SHAPE}\npub fn readValue(n: int): int {{\n    var result = 0\n    \
         transaction {{\n        place slot = ^counters[n]\n        \
         if exists(slot) {{\n            slot.label = \"seen\"\n        }}\n        \
         result = ^counters[n].value ?? 0\n    }}\n    return result\n}}\n"
    )
}

fn compile(source: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = vec![marrow_project::CapturedFile::new(
        "src/main.mw".to_string(),
        source.as_bytes().to_vec(),
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

fn provision(store: &Path, image: &VerifiedImage) {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    let report = ProvisionReport::new(store, image, &schemas);
    let approval = ProvisionApproval::accept(&report);
    provision_image(store, image, schemas, sites, &approval).expect("provision");
}

fn attach_image(store: &Path, image: &VerifiedImage) -> Result<AttachOutcome, LifecycleError> {
    let (schemas, sites) = marrow_vm::derive_store_schemas(image).expect("flat-executable");
    attach(store, image, schemas, sites)
}

fn scratch() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "marrow-g03-audit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    std::fs::create_dir_all(&base).expect("scratch base");
    base.join("store")
}

#[test]
fn effective_authority_is_demand_ceiling_grant_and_the_reserved_principal_slot() {
    let read_only = compile(&source_read_only());
    let broadened = compile(&source_broadened());
    let store = scratch();

    // The store's accepted ceiling is the read-only image's demand union.
    provision(&store, &read_only);

    // TERM 1 — demand ∩ ceiling: the broadened demand exceeds the accepted ceiling and is
    // refused at attach, before any engine call. It never reaches the grant or principal terms.
    let head_before = std::fs::read(store.join("head")).expect("head");
    match attach_image(&store, &broadened) {
        Err(LifecycleError::DemandExceedsCeiling(refusal)) => {
            assert_eq!(refusal.code(), "store.demand_exceeds_ceiling");
        }
        Err(other) => panic!(
            "term 1: the broadened demand must be refused at the ceiling, got: {}",
            other.code()
        ),
        Ok(_) => panic!("term 1: the broadened demand must be refused, not admitted"),
    }
    assert_eq!(
        head_before,
        std::fs::read(store.join("head")).expect("head"),
        "term 1: the ceiling refusal made zero engine calls (the head is byte-unchanged)",
    );

    // The read-only image is admitted (its demand fits the ceiling); it opens the store so the
    // remaining terms are checked at the kernel session over a real native handle.
    let mut opened = match attach_image(&store, &read_only) {
        Ok(AttachOutcome::AlreadyActive(opened)) => opened,
        Ok(AttachOutcome::Rebound { store, .. }) => store,
        Err(err) => panic!(
            "the covering image must open the store, got: {}",
            err.code()
        ),
    };

    let read = DemandCoverage {
        read: true,
        write: false,
    };
    let write = DemandCoverage {
        read: true,
        write: true,
    };

    // TERM 2 — ∩ grant (attenuation): a read-only grant denies a mutating demand at session
    // open, even over a store whose engine could write. Attenuation gates after the ceiling.
    let read_only_grant = InvocationGrant {
        read: true,
        write: false,
    };
    assert!(
        matches!(
            opened.store.txn_session(read_only_grant, write),
            Err(SessionError::Denied)
        ),
        "term 2: a read-only grant must deny a mutating demand at session open",
    );

    // TERM 3 — ∩ principal (reserved ⊤): the fourth term narrows the effective authority to
    // itself, adding and removing nothing, so the order is demand → ceiling → grant → principal
    // with the principal as the identity today.
    let effective_read = DemandCoverage {
        read: true,
        write: false,
    };
    assert_eq!(
        PrincipalPredicate::Any.narrow(effective_read),
        effective_read,
        "term 3: the reserved principal slot is ⊤ (identity): it never adds or removes an atom",
    );

    // TERM 4 — all admit: a full grant, a demand within the ceiling, and the ⊤ principal open a
    // read session that observes the store.
    let full = InvocationGrant::full_store();
    assert!(
        PrincipalPredicate::Any.narrow(read) == read
            && opened.store.read_session(full, read).is_ok(),
        "term 4: when all four terms admit, the session opens and observes the store",
    );

    let _ = std::fs::remove_dir_all(store.parent().expect("parent"));
}
