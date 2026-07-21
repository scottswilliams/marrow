//! The lifecycle actor over a real compiled durable image: binding-facts derivation, the
//! head-map ↔ kernel-numbering agreement, and the attach classifier (already-active, the
//! binding-only rebind, and the typed contract-changed refusals).

use std::path::{Path, PathBuf};

use marrow_lifecycle::{
    AttachOutcome, ChangedFact, EngineKind, LifecycleError, LogicalHead, ProvisionRequest,
    StoreEnvelope, StoreInstanceId, active_binding, attach, head_map, provision,
    reopen_and_classify,
};
use marrow_verify::{VerifiedImage, verify};

/// The base durable program: a `counters` root of `Counter` resources (a required `value`
/// and a sparse `label`), keyed by `id: int`, with one read-only export.
const BASE_SOURCE: &str = r#"resource Counter {
    required value: int
    label: string
}

store ^counters[id: int]: Counter

pub fn readValue(n: int): int {
    return ^counters[n].value ?? 0
}
"#;

/// The identity ledger for [`BASE_SOURCE`]: the application, the `Counter` product, its two
/// fields, the `counters` root, and its key column.
const BASE_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Counter 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Counter.value 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Counter.label 0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f\n\
     id root counters 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key counters.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
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

fn schemas_of(
    image: &VerifiedImage,
) -> (
    Vec<marrow_kernel::durable::StoreSchema>,
    Vec<marrow_kernel::durable::SiteSpec>,
) {
    marrow_vm::derive_store_schemas(image).expect("the base image is flat-executable")
}

/// A unique scratch store directory, removed on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "marrow-lifecycle-actor-{tag}-{}-{}",
            std::process::id(),
            now_nonce(),
        ));
        std::fs::create_dir_all(&base).expect("create scratch base");
        Self {
            dir: base.join("store"),
        }
    }
    fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(parent) = self.dir.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

fn now_nonce() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// Provision a fresh store at `dir` bound to `image`.
fn provision_from(dir: &Path, image: &VerifiedImage) -> StoreInstanceId {
    let (schemas, sites) = schemas_of(image);
    let instance = StoreInstanceId::draw().expect("entropy");
    let envelope = StoreEnvelope {
        instance,
        writer_toolchain: "0.1.0".to_string(),
        engine_kind: EngineKind::Redb,
        engine_format_version: 1,
    };
    let head = LogicalHead::provision(active_binding(image), head_map(image).expect("head map"));
    provision(
        dir,
        ProvisionRequest {
            envelope,
            head,
            schemas,
            sites,
        },
    )
    .expect("provision");
    instance
}

#[test]
fn active_binding_and_head_map_derive_from_the_image() {
    let image = compile(BASE_SOURCE, BASE_IDS);
    let binding = active_binding(&image);
    // The binding facts are the image's real identities, not placeholders.
    assert_ne!(binding.durable_contract, [0u8; 32]);
    assert_ne!(binding.interface, [0u8; 32]);
    assert_ne!(binding.ceiling, [0u8; 32]);
    assert_ne!(binding.image_id, [0u8; 32]);

    // The head map numbers the three cell-key nodes: the `counters` root and its two fields.
    let map = head_map(&image).expect("head map");
    assert_eq!(map.len(), 3, "root + two fields");
    assert_eq!(map.next_number(), 3);
}

/// The head-map numbering agrees node-for-node with the kernel's `number_store`: both walk
/// the durable graph in the same split pre-order, so they assign the same count of nodes.
/// This is the cross-crate enforcement artifact against pre-order drift (FR01 §3).
#[test]
fn head_map_numbering_agrees_with_the_kernel() {
    let image = compile(BASE_SOURCE, BASE_IDS);
    let (schemas, _sites) = schemas_of(&image);
    let numbering = marrow_kernel::durable::number_store(&schemas);

    let kernel_node_count: usize = numbering
        .iter()
        .map(|root| {
            1 + root.fields.len()
                + root
                    .groups
                    .iter()
                    .map(|group| 1 + group.fields.len())
                    .sum::<usize>()
                + branch_node_count(&root.branches)
        })
        .sum();

    let map = head_map(&image).expect("head map");
    assert_eq!(
        map.len(),
        kernel_node_count,
        "the head-map walk must visit exactly the kernel's cell-key nodes",
    );
    // The head map's numbers are exactly 0..count in the kernel's order.
    for (i, entry) in map.entries().iter().enumerate() {
        assert_eq!(entry.number, i as u32);
    }
}

fn branch_node_count(branches: &[marrow_kernel::durable::BranchNumbering]) -> usize {
    branches
        .iter()
        .map(|branch| 1 + branch.fields.len() + branch_node_count(&branch.branches))
        .sum()
}

#[test]
fn attach_to_the_same_image_is_already_active() {
    let scratch = Scratch::new("already-active");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    let (schemas, sites) = schemas_of(&image);
    match attach(scratch.dir(), &image, schemas, sites).expect("attach") {
        AttachOutcome::AlreadyActive(store) => drop(store),
        AttachOutcome::Rebound { .. } => panic!("an identical image must be already-active"),
    }
}

#[test]
fn a_body_only_edit_is_a_binding_only_rebind() {
    let scratch = Scratch::new("rebind");
    let image = compile(BASE_SOURCE, BASE_IDS);
    let instance = provision_from(scratch.dir(), &image);
    let original = active_binding(&image);

    // A body-only edit: the fallback default changes, so the image bytes differ, but the
    // export signature, the durable contract, and the ceiling are all preserved.
    let edited_source = BASE_SOURCE.replace("?? 0", "?? 1");
    let edited = compile(&edited_source, BASE_IDS);
    let edited_binding = active_binding(&edited);
    assert_ne!(
        edited_binding.image_id, original.image_id,
        "the code changed"
    );
    assert!(
        original.facts_equal(&edited_binding),
        "the facts are preserved"
    );

    let (schemas, sites) = schemas_of(&edited);
    let receipt = match attach(scratch.dir(), &edited, schemas, sites).expect("attach") {
        AttachOutcome::Rebound { store, receipt } => {
            drop(store);
            receipt
        }
        AttachOutcome::AlreadyActive(_) => panic!("a body edit must rebind, not be already-active"),
    };
    assert_eq!(receipt.instance, instance);
    assert_eq!(receipt.new_image_id, edited_binding.image_id);

    // The rebind persisted: reopening reads the new image as the active binding, and the head
    // map (durable contract unchanged) is preserved.
    let opened = open_head(scratch.dir(), &edited);
    assert_eq!(opened.binding.image_id, edited_binding.image_id);
    assert_eq!(
        opened.head_map,
        head_map(&image).expect("head map"),
        "the head map is preserved across a binding-only rebind",
    );
}

/// Reopen the store's head via a fresh open, returning the persisted logical head.
fn open_head(dir: &Path, image: &VerifiedImage) -> LogicalHead {
    let (schemas, sites) = schemas_of(image);
    let opened = marrow_lifecycle::open(dir, schemas, sites).expect("open");
    opened.head
}

#[test]
fn adding_an_export_is_a_typed_interface_refusal() {
    let scratch = Scratch::new("iface");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    // A new pure export changes the exported interface, not the durable contract or ceiling.
    let extended = format!("{BASE_SOURCE}\npub fn two(): int {{\n    return 2\n}}\n");
    let changed = compile(&extended, BASE_IDS);

    let (schemas, sites) = schemas_of(&changed);
    match attach(scratch.dir(), &changed, schemas, sites) {
        Err(LifecycleError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::Interface);
            assert_eq!(refusal.code(), "store.contract_changed");
            assert_ne!(refusal.code(), "store.corruption");
        }
        Err(other) => panic!("expected an interface refusal, got code {}", other.code()),
        Ok(_) => panic!("an interface change must be refused, but attach succeeded"),
    }
}

#[test]
fn changing_the_durable_contract_is_a_typed_refusal() {
    let scratch = Scratch::new("contract");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    // Promote the sparse `label` field to required — the same durable node (same ledger id),
    // but a changed required flag, which is part of the durable contract. The exported
    // interface (readValue) and the ceiling are unchanged, so only the durable contract
    // differs.
    let evolved_source = BASE_SOURCE.replace("    label: string\n", "    required label: string\n");
    let changed = compile(&evolved_source, BASE_IDS);

    let (schemas, sites) = schemas_of(&changed);
    match attach(scratch.dir(), &changed, schemas, sites) {
        Err(LifecycleError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::DurableContract);
            assert_eq!(refusal.code(), "store.contract_changed");
        }
        Err(other) => panic!(
            "expected a durable-contract refusal, got code {}",
            other.code()
        ),
        Ok(_) => panic!("a durable-contract change must be refused, but attach succeeded"),
    }
}

#[test]
fn reopen_classifies_an_uncommitted_token_as_complete_old() {
    let scratch = Scratch::new("reopen");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    // A token that never landed as a witness classifies complete-old: the interrupted commit
    // did not complete. (A complete-new classification requires a committed write session,
    // exercised on the terminal companion path at F02b.)
    let (schemas, sites) = schemas_of(&image);
    let classification =
        reopen_and_classify(scratch.dir(), [0x55; 16], schemas, sites).expect("reopen");
    assert_eq!(classification, marrow_kernel::durable::Reopen::CompleteOld);
}
