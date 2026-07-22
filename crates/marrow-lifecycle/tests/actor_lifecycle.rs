//! The lifecycle actor over a real compiled durable image: binding-facts derivation, the
//! head-map ↔ kernel-numbering agreement, and the attach classifier (already-active, the
//! binding-only rebind, and the typed contract-changed refusals).

use std::path::{Path, PathBuf};

use marrow_lifecycle::{
    AttachOutcome, ChangedFact, EngineKind, HEAD_FILE, LifecycleError, LogicalHead,
    ProvisionRequest, StoreEnvelope, StoreInstanceId, active_binding, attach, head_map, open,
    provision,
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
    let head = LogicalHead::provision(
        active_binding(image),
        marrow_lifecycle::accepted_ceiling(image),
        head_map(image).expect("head map"),
    );
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
    assert_ne!(binding.image_id, [0u8; 32]);
    // The accepted ceiling is a non-empty atom-set payload derived from the image demand.
    assert!(!marrow_lifecycle::accepted_ceiling(&image).is_empty());

    // The head map numbers the three cell-key nodes: the `counters` root and its two fields.
    let map = head_map(&image).expect("head map");
    assert_eq!(map.len(), 3, "root + two fields");
    assert_eq!(map.next_number(), 3);
}

/// A durable program exercising every split-order decision point across more than one shape:
/// **two roots** (`books`, `tags` — the outer declaration-order loop), a resource with **two
/// top-level fields** (field order), **two sibling groups** each of one field (group order and
/// the group-then-its-members split), and a **nested branch** (`notes` carrying a `replies`
/// sub-branch — the recursive branch descent). A single-shape fixture would leave the ordering
/// and recursion split — the only place the kernel and head-map walks could diverge —
/// under-driven.
const GRAPH_SOURCE: &str = r#"resource Tag {
    required name: string
}

resource Book {
    required title: string
    subtitle: string

    details {
        pages: int
    }

    meta {
        isbn: string
    }

    notes[noteId: string] {
        required body: string

        replies[replyId: string] {
            required text: string
        }
    }
}

store ^books[id: int]: Book
store ^tags[id: int]: Tag

pub fn readTitle(id: int): string {
    return ^books[id].title ?? "?"
}
"#;

/// The identity ledger for [`GRAPH_SOURCE`]: every durable anchor — two products, every field,
/// the two groups, the `notes` branch and its nested `replies` sub-branch (each a `root`-
/// anchored placement, keys and fields path-qualified through the branch chain), and both
/// store roots with their keys.
const GRAPH_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id field Book.subtitle 1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e1e\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id group Book.meta 22222222222222222222222222222222\n\
     id field Book.meta.isbn 23232323232323232323232323232323\n\
     id root Book.notes 30303030303030303030303030303030\n\
     id key Book.notes.noteId 31313131313131313131313131313131\n\
     id field Book.notes.body 32323232323232323232323232323232\n\
     id root Book.notes.replies 33333333333333333333333333333333\n\
     id key Book.notes.replies.replyId 34343434343434343434343434343434\n\
     id field Book.notes.replies.text 35353535353535353535353535353535\n\
     id product Tag 40404040404040404040404040404040\n\
     id field Tag.name 41414141414141414141414141414141\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     id root tags 4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b4b\n\
     id key tags.id 4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c4c\n\
     high-water 0\n\
     end\n";

/// The head-map numbering agrees node-for-node with the kernel's `number_store`: both walk
/// the durable graph in the same canonical split pre-order, so they assign the same *kind* to
/// each cell-key node at each position. This is the cross-crate enforcement artifact against
/// pre-order drift between the two independent numbering owners (FR01 §3): a divergence in the
/// order of, or the fields/groups/branches split within, either walk fails here. The fixture
/// drives multi-root order, sibling-field and sibling-group order, and recursive nested-branch
/// descent — every point where the two independent walks could disagree.
#[test]
fn head_map_numbering_agrees_with_the_kernel_node_for_node() {
    use marrow_verify::SemanticNodeKind;

    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    let (schemas, _sites) = schemas_of(&image);

    // The kernel's numbering, flattened into its cell-key node kinds in numbering order.
    let numbering = marrow_kernel::durable::number_store(&schemas);
    let mut kernel_order: Vec<SemanticNodeKind> = Vec::new();
    for root in &numbering {
        kernel_order.push(SemanticNodeKind::Root);
        kernel_order.extend(root.fields.iter().map(|_| SemanticNodeKind::Field));
        for group in &root.groups {
            kernel_order.push(SemanticNodeKind::Group);
            kernel_order.extend(group.fields.iter().map(|_| SemanticNodeKind::Field));
        }
        flatten_branches(&root.branches, &mut kernel_order);
    }

    // The lifecycle head-map walk's node kinds, in its numbering order.
    let lifecycle_order = marrow_lifecycle::head_map_node_order(&image);

    assert_eq!(
        lifecycle_order, kernel_order,
        "the head-map split-order walk must agree node-for-node with the kernel numbering",
    );
    // And the persisted head map has exactly one entry per node, numbered 0..n in that order.
    let map = head_map(&image).expect("head map");
    assert_eq!(map.len(), kernel_order.len());
    let count = |kind| kernel_order.iter().filter(|k| **k == kind).count();
    assert!(
        count(SemanticNodeKind::Root) >= 2,
        "multi-root not exercised"
    );
    assert!(
        count(SemanticNodeKind::Group) >= 2,
        "sibling groups not exercised",
    );
    assert!(
        count(SemanticNodeKind::Branch) >= 2,
        "nested branch not exercised",
    );
    for (i, entry) in map.entries().iter().enumerate() {
        assert_eq!(entry.number, i as u32);
    }
}

fn flatten_branches(
    branches: &[marrow_kernel::durable::BranchNumbering],
    out: &mut Vec<marrow_verify::SemanticNodeKind>,
) {
    use marrow_verify::SemanticNodeKind;
    for branch in branches {
        out.push(SemanticNodeKind::Branch);
        out.extend(branch.fields.iter().map(|_| SemanticNodeKind::Field));
        flatten_branches(&branch.branches, out);
    }
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
    let opened = open(dir, schemas, sites).expect("open");
    opened.head
}

/// The fast-path crash matrix (F02b): a kill during a binding-only rebind, after the head
/// (the active-binding commit point) is renamed into place but before the envelope (writer
/// provenance) is rewritten, recovers to the complete NEW binding — the store reopens cleanly
/// and its active binding is the new image, with the stale envelope forensic-only. A kill
/// before the head rename leaves the OLD binding, since a single-file rename is atomic (each
/// artifact is wholly old or wholly new, never torn); this test exercises the new-binding leg,
/// the one the ordering makes non-trivial.
#[test]
fn a_crash_between_head_and_envelope_commit_recovers_to_the_new_binding() {
    let scratch = Scratch::new("crash-rebind");
    let image_a = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image_a);

    // A body-only edit: same durable contract, interface, and ceiling; different code.
    let edited = compile(&BASE_SOURCE.replace("?? 0", "?? 1"), BASE_IDS);
    let binding_b = active_binding(&edited);

    // Simulate the crash: open the store, stamp the new binding into the head (the commit
    // point), and write only the head back — leaving the old envelope, exactly the on-disk
    // state a kill between the head rename and the envelope rewrite leaves.
    let (schemas, sites) = schemas_of(&image_a);
    let mut opened = open(scratch.dir(), schemas, sites).expect("open");
    opened.head.binding = binding_b;
    let crashed_head = opened.head.encode();
    drop(opened); // release the single-owner lock before reopening
    std::fs::write(scratch.dir().join(HEAD_FILE), &crashed_head).expect("write crashed head");

    // Reopen: the store is complete and runnable, and the active binding is the new image B.
    let reopened = open_head(scratch.dir(), &edited);
    assert_eq!(
        reopened.binding.image_id, binding_b.image_id,
        "reopen after the crash yields the new binding (the head is the commit point)",
    );
    assert!(
        reopened.binding.facts_equal(&binding_b),
        "the recovered binding facts match the new image",
    );
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
