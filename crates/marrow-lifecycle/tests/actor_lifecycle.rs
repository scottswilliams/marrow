//! The lifecycle actor over a real compiled durable image: binding-facts derivation, the
//! head-map ↔ kernel-numbering agreement, and the attach classifier (already-active, the
//! binding-only rebind, and the typed contract-changed refusals).

use std::path::{Path, PathBuf};

#[path = "../../marrow-compile/tests/common/source_projection.rs"]
mod source_projection;

use marrow_lifecycle::{
    AttachOutcome, ChangedFact, EngineKind, HEAD_FILE, LifecycleError, LogicalHead,
    PinDisagreement, ProvisionRequest, StoreEnvelope, StoreInstanceId, active_binding, attach,
    head_map, open, provision,
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

fn projection_of(image: &VerifiedImage) -> marrow_kernel::durable::StoreProjection {
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
    let projection = projection_of(image);
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
            projection,
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
/// the durable graph in the same canonical split pre-order, so position `i` in both walks
/// must be the *same node* — same kind **and same ledger identity**, resolved here through
/// [`GRAPH_IDS`]'s explicit anchor table. This is the cross-crate enforcement artifact
/// against pre-order drift between the two independent numbering owners (FR01 §3): a
/// divergence in the order of, or the fields/groups/branches split within, either walk fails
/// here — including two same-kind siblings swapped in only one walk, which a kind-only
/// comparison would miss while the head map bound their ledger ids to each other's numbers.
/// The fixture drives multi-root order, sibling-field and sibling-group order, and recursive
/// nested-branch descent — every point where the two independent walks could disagree.
#[test]
fn head_map_numbering_agrees_with_the_kernel_node_for_node() {
    use marrow_verify::SemanticNodeKind::{Branch, Field, Group, Root};

    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    let projection = projection_of(&image);

    // The kernel's numbering order, flattened by walking the projection's schemas in
    // lockstep (the numbering mirrors the schema structurally), each node resolved to the
    // ledger id GRAPH_IDS anchors it to.
    let mut kernel_order: Vec<(marrow_verify::SemanticNodeKind, marrow_image::LedgerIdBytes)> =
        Vec::new();
    for schema in projection.roots() {
        let root = schema.root_name();
        kernel_order.push((Root, graph_id(&[root])));
        for field in schema.fields() {
            kernel_order.push((Field, graph_id(&[root, field.name()])));
        }
        for group in schema.groups() {
            kernel_order.push((Group, graph_id(&[root, group.name()])));
            for field in group.fields() {
                kernel_order.push((Field, graph_id(&[root, group.name(), field.name()])));
            }
        }
        flatten_branches(root, &mut Vec::new(), schema.branches(), &mut kernel_order);
    }

    // The kernel's own number assignment ties to this same structural order: `number_store`
    // over the derived projection allocates exactly 0..n-1 when read in the structural walk
    // (each root, its fields, its groups and their fields, its branches recursively), so the
    // name-anchored order above is also the kernel's allocation order — the third leg the
    // runtime pin comparison consumes.
    let numbering = marrow_kernel::durable::number_store(&projection);
    let mut kernel_numbers: Vec<u32> = Vec::new();
    for root in &numbering {
        kernel_numbers.push(root.root());
        kernel_numbers.extend_from_slice(root.fields());
        for group in root.groups() {
            kernel_numbers.push(group.number());
            kernel_numbers.extend_from_slice(group.fields());
        }
        flatten_branch_numbers(root.branches(), &mut kernel_numbers);
    }
    assert_eq!(
        kernel_numbers,
        (0..kernel_order.len() as u32).collect::<Vec<_>>(),
        "number_store allocates dense pre-order numbers in the structural walk order",
    );

    // The lifecycle head-map walk's (kind, ledger id) pairs, in its numbering order.
    let lifecycle_order = marrow_lifecycle::head_map_node_order(&image);

    assert_eq!(
        lifecycle_order, kernel_order,
        "the head-map split-order walk must agree node-for-node with the kernel numbering",
    );
    // And the persisted head map has exactly one entry per node, numbered 0..n in that
    // order, binding exactly the ids the kernel walk expects at each number.
    let map = head_map(&image).expect("head map");
    assert_eq!(map.len(), kernel_order.len());
    let count = |kind| kernel_order.iter().filter(|(k, _)| *k == kind).count();
    assert!(count(Root) >= 2, "multi-root not exercised");
    assert!(count(Group) >= 2, "sibling groups not exercised");
    assert!(count(Branch) >= 2, "nested branch not exercised");
    for (i, entry) in map.entries().iter().enumerate() {
        assert_eq!(entry.number, i as u32);
        assert_eq!(
            entry.ledger_id, kernel_order[i].1,
            "head-map number {i} binds a different node than the kernel walk",
        );
    }
}

fn flatten_branch_numbers(
    branches: &[marrow_kernel::durable::BranchNumbering],
    out: &mut Vec<u32>,
) {
    for branch in branches {
        out.push(branch.number());
        out.extend_from_slice(branch.fields());
        flatten_branch_numbers(branch.branches(), out);
    }
}

fn flatten_branches(
    root: &str,
    path: &mut Vec<String>,
    branches: &[marrow_kernel::durable::BranchSchema],
    out: &mut Vec<(marrow_verify::SemanticNodeKind, marrow_image::LedgerIdBytes)>,
) {
    use marrow_verify::SemanticNodeKind::{Branch, Field};
    for branch in branches {
        path.push(branch.name().to_string());
        let mut segments: Vec<&str> = vec![root];
        segments.extend(path.iter().map(String::as_str));
        out.push((Branch, graph_id(&segments)));
        for field in branch.fields() {
            let mut segments = segments.clone();
            segments.push(field.name());
            out.push((Field, graph_id(&segments)));
        }
        flatten_branches(root, path, branch.branches(), out);
        path.pop();
    }
}

/// Resolve a kernel-walk node — named by its store root and member-name path — to the
/// ledger id [`GRAPH_IDS`] anchors it to. The store roots are occurrence anchors; their
/// members are declaration anchors under the occurrence's product. Explicit per fixture, so
/// a wrong binding cannot hide in a clever shared renderer.
fn graph_id(segments: &[&str]) -> marrow_image::LedgerIdBytes {
    let byte = match segments {
        ["books"] => 0x0b,
        ["books", "title"] => 0x0e,
        ["books", "subtitle"] => 0x1e,
        ["books", "details"] => 0x20,
        ["books", "details", "pages"] => 0x21,
        ["books", "meta"] => 0x22,
        ["books", "meta", "isbn"] => 0x23,
        ["books", "notes"] => 0x30,
        ["books", "notes", "body"] => 0x32,
        ["books", "notes", "replies"] => 0x33,
        ["books", "notes", "replies", "text"] => 0x35,
        ["tags"] => 0x4b,
        ["tags", "name"] => 0x41,
        other => panic!("no GRAPH_IDS anchor for kernel walk node {other:?}"),
    };
    marrow_image::LedgerIdBytes::from_bytes([byte; 16])
}

#[test]
fn attach_to_the_same_image_is_already_active() {
    let scratch = Scratch::new("already-active");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    let projection = projection_of(&image);
    match attach(scratch.dir(), &image, projection).expect("attach") {
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

    let projection = projection_of(&edited);
    let receipt = match attach(scratch.dir(), &edited, projection).expect("attach") {
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
    let projection = projection_of(image);
    let opened = open(dir, projection).expect("open");
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
    let projection = projection_of(&image_a);
    let mut opened = open(scratch.dir(), projection).expect("open");
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

/// Rewrite the store's head at `dir` with the same binding and ceiling as `image` but a
/// *permuted* head map: the same ledger ids with the first and last swapped, so two nodes
/// carry each other's cell numbers. The permuted map is still a valid bijection and the
/// rewritten head reseals, so decode admits it — only the pin comparison can catch the
/// disagreement. Returns the first walked node's ledger id and the number the permutation
/// pins it to (the last number).
fn permute_persisted_pin(dir: &Path, image: &VerifiedImage) -> (marrow_image::LedgerIdBytes, u32) {
    let map = head_map(image).expect("head map");
    let mut ids: Vec<marrow_image::LedgerIdBytes> =
        map.entries().iter().map(|entry| entry.ledger_id).collect();
    let last = ids.len() - 1;
    ids.swap(0, last);
    let permuted = marrow_lifecycle::HeadMap::assign(&ids).expect("a permuted bijection assigns");
    let forged = LogicalHead::provision(
        active_binding(image),
        marrow_lifecycle::accepted_ceiling(image),
        permuted,
    );
    std::fs::write(dir.join(HEAD_FILE), forged.encode()).expect("write permuted head");
    (ids[last], last as u32)
}

/// The pin family covers every serving outcome: an attach serves a store only as
/// already-active or as a binding-only rebind, and both arms are fenced by a permuted-pin
/// fixture below. A new [`AttachOutcome`] variant fails this match until the family covers
/// it too.
fn _pin_family_covers_every_serving_outcome(outcome: AttachOutcome) {
    match outcome {
        AttachOutcome::AlreadyActive(_) => (),
        AttachOutcome::Rebound { .. } => (),
    }
}

/// The head-map pin bites (FR01 §3): a store is never served under a numbering that
/// disagrees with its persisted ledger-id ↔ cell-number bijection. A permuted bijection —
/// the exact readdressing hazard where ledger id X's bytes would be served as id Y's value —
/// is refused at attach with a typed fail-closed error naming the pin's first disagreeing
/// binding, on the already-active arm the same image would otherwise serve through.
#[test]
fn a_store_with_a_permuted_head_map_pin_is_refused_at_attach() {
    let scratch = Scratch::new("pin-permuted");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    let (first_id, last_number) = permute_persisted_pin(scratch.dir(), &image);

    let projection = projection_of(&image);
    match attach(scratch.dir(), &image, projection) {
        Err(LifecycleError::HeadMapPin(refusal)) => {
            assert_eq!(
                refusal.code(),
                "store.corruption",
                "fail-closed, recovery-shaped"
            );
            // The typed payload names the pin's first disagreement in derived walk order:
            // the first walked node (derived number 0) is pinned to the last number.
            assert_eq!(
                refusal.disagreement,
                PinDisagreement::Binding {
                    ledger_id: first_id,
                    persisted: Some(last_number),
                    derived: Some(0),
                },
            );
        }
        Err(other) => panic!("expected the pin refusal, got code {}", other.code()),
        Ok(_) => panic!(
            "a store whose persisted head-map pin disagrees with the derived numbering must \
             be refused, but attach served it"
        ),
    }
}

/// The pin refusal precedes any engine call: with the engine file replaced by garbage — an
/// engine open would fail loudly — a permuted pin still surfaces as the pin refusal, so the
/// disagreement is decided strictly before the engine (and therefore before any read or
/// mutation) is reached.
#[test]
fn the_pin_refusal_precedes_any_engine_call() {
    let scratch = Scratch::new("pin-before-engine");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    permute_persisted_pin(scratch.dir(), &image);
    std::fs::write(
        scratch.dir().join(marrow_lifecycle::ENGINE_FILE),
        b"not an engine",
    )
    .expect("corrupt the engine file");

    let projection = projection_of(&image);
    match attach(scratch.dir(), &image, projection) {
        Err(LifecycleError::HeadMapPin(_)) => {}
        Err(other) => panic!(
            "the pin must refuse before the engine is touched, got code {}",
            other.code()
        ),
        Ok(_) => panic!("a permuted pin over a garbage engine was served"),
    }
}

/// The rebind arm is fenced too: a body-only edit (the binding-only rebind case) against a
/// permuted pin is refused without rewriting the head or envelope, and the refusal releases
/// the single-owner lock — restoring the true head lets the same rebind commit.
#[test]
fn a_rebind_over_a_permuted_pin_is_refused_without_a_write() {
    let scratch = Scratch::new("pin-rebind");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    let true_head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read true head");
    permute_persisted_pin(scratch.dir(), &image);

    let before_head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head");
    let before_envelope =
        std::fs::read(scratch.dir().join(marrow_lifecycle::ENVELOPE_FILE)).expect("read envelope");

    // A body-only edit: same durable contract and interface, different image bytes.
    let edited = compile(&GRAPH_SOURCE.replace("?? \"?\"", "?? \"!\""), GRAPH_IDS);
    assert!(
        active_binding(&image).facts_equal(&active_binding(&edited)),
        "the edit is binding-only"
    );
    match attach(scratch.dir(), &edited, projection_of(&edited)) {
        Err(LifecycleError::HeadMapPin(_)) => {}
        Err(other) => panic!("expected the pin refusal, got code {}", other.code()),
        Ok(_) => panic!("a rebind over a permuted pin was served"),
    }
    assert_eq!(
        std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head"),
        before_head,
        "the refusal rewrites nothing",
    );
    assert_eq!(
        std::fs::read(scratch.dir().join(marrow_lifecycle::ENVELOPE_FILE)).expect("read envelope"),
        before_envelope,
        "the refusal rewrites nothing",
    );

    // The refusal released the lock: with the true pin restored, the same rebind commits.
    std::fs::write(scratch.dir().join(HEAD_FILE), &true_head).expect("restore the true head");
    match attach(scratch.dir(), &edited, projection_of(&edited)).expect("attach") {
        AttachOutcome::Rebound { store, .. } => drop(store),
        AttachOutcome::AlreadyActive(_) => panic!("a body edit must rebind"),
    }
}

/// A changed durable contract is a different graph whose numbering legitimately differs, so
/// the pin comparison does not preempt the typed contract-changed refusal: over a permuted
/// pin, an image with an evolved contract is still refused as `store.contract_changed` — and
/// the store is not served on that path either.
#[test]
fn a_contract_change_over_a_permuted_pin_stays_a_contract_refusal() {
    let scratch = Scratch::new("pin-contract");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    permute_persisted_pin(scratch.dir(), &image);

    // The same durable node set (same ledger ids) with one field promoted to required — a
    // durable-contract change that leaves the numbering walk identical.
    let evolved = compile(
        &GRAPH_SOURCE.replace("    subtitle: string\n", "    required subtitle: string\n"),
        GRAPH_IDS,
    );
    match attach(scratch.dir(), &evolved, projection_of(&evolved)) {
        Err(LifecycleError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::DurableContract);
        }
        Err(other) => panic!("expected the contract refusal, got code {}", other.code()),
        Ok(_) => panic!("a contract change must be refused"),
    }
}

/// The pin's high-water is part of the comparison: a head whose bindings all agree but whose
/// never-reuse high-water is inflated — forged by byte surgery and validly resealed, claiming
/// numbers were used and retired where the derivation retires nothing — is a typed high-water
/// disagreement.
#[test]
fn an_inflated_pin_high_water_is_a_typed_disagreement() {
    let scratch = Scratch::new("pin-high-water");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    let node_count = head_map(&image).expect("head map").len() as u32;

    // The head map's high-water u32 sits right after the fixed head prefix:
    // magic(4)+ver(1)+imgfmt(1)+3×id(32)+commit(8)+ddig(32)+ddpos(8) = 150.
    let head_path = scratch.dir().join(HEAD_FILE);
    let mut bytes = std::fs::read(&head_path).expect("read head");
    let map_start = 4 + 1 + 1 + 32 * 3 + 8 + 32 + 8;
    bytes[map_start..map_start + 4].copy_from_slice(&(node_count + 7).to_be_bytes());
    let body_len = bytes.len() - 32;
    let resealed = marrow_image::StoreHeadDigest::compute(&bytes[..body_len]);
    bytes[body_len..].copy_from_slice(resealed.bytes());
    std::fs::write(&head_path, &bytes).expect("write forged head");

    match attach(scratch.dir(), &image, projection_of(&image)) {
        Err(LifecycleError::HeadMapPin(refusal)) => {
            assert_eq!(
                refusal.disagreement,
                PinDisagreement::HighWater {
                    persisted: node_count + 7,
                    derived: node_count,
                },
            );
        }
        Err(other) => panic!("expected the pin refusal, got code {}", other.code()),
        Ok(_) => panic!("an inflated high-water must be refused"),
    }
}

/// The pin bites on projection-derivation drift, the kernel-side hazard: a projection that
/// orders the store differently than the provisioning toolchain did (simulated here by
/// re-deriving the same schemas with the roots swapped) yields different (ledger id → cell
/// number) pairs, and attach refuses before any engine call. Kernel numbers are dense over
/// any projection shape, so only the name pairing — not walk position — catches this.
#[test]
fn a_drifted_projection_derivation_is_refused_by_the_pin() {
    let scratch = Scratch::new("pin-drifted-projection");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);

    let derived = projection_of(&image);
    let mut builder = marrow_kernel::durable::StoreProjection::builder();
    for schema in derived.roots().iter().rev() {
        builder.root(schema.clone());
    }
    let drifted = builder
        .finish()
        .expect("the swapped-root projection builds");

    // Under the drifted derivation the tags root would be served as cell number 0, while
    // the persisted pin binds it to its provisioned number — the first disagreement in the
    // drifted walk order.
    let map = head_map(&image).expect("head map");
    let tags_root = marrow_image::LedgerIdBytes::from_bytes([0x4b; 16]);
    let pinned_tags = map
        .number_of(&tags_root)
        .expect("the pin binds the tags root");
    match attach(scratch.dir(), &image, drifted) {
        Err(LifecycleError::HeadMapPin(refusal)) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Binding {
                ledger_id: tags_root,
                persisted: Some(pinned_tags),
                derived: Some(0),
            },
        ),
        Err(other) => panic!("expected the pin refusal, got code {}", other.code()),
        Ok(_) => panic!("a drifted projection derivation readdresses cells and must refuse"),
    }
}

/// A store-schema node the image does not name is a typed fail-closed refusal: no ledger
/// identity can be paired with its kernel number, so no pin can be derived at all.
#[test]
fn an_unnamed_store_node_is_a_typed_refusal() {
    use marrow_kernel::codec::value::ScalarKind;

    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    let schema = marrow_kernel::durable::StoreSchemaBuilder::root("phantom", vec![ScalarKind::Int])
        .finish()
        .expect("a rootonly schema builds");
    let mut builder = marrow_kernel::durable::StoreProjection::builder();
    builder.root(schema);
    let foreign = builder.finish().expect("the projection builds");

    let map = head_map(&image).expect("head map");
    match marrow_lifecycle::verify_head_map_pin(&image, &foreign, &map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Unnamed {
                place: "^phantom".to_string()
            },
        ),
        Ok(()) => panic!("an unnamed store node must refuse"),
    }
}

/// The honest ordering of the contract-changed path: the pin comparison is scoped to an
/// unchanged durable contract, and the contract-changed classification runs after the
/// engine's physical open (numbering-independent access, no session). A failing engine on
/// that path therefore surfaces as its own open error — never as `store.contract_changed`,
/// and never as a served store.
#[test]
fn a_changed_contract_over_a_garbage_engine_is_an_open_error() {
    let scratch = Scratch::new("contract-garbage-engine");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    std::fs::write(
        scratch.dir().join(marrow_lifecycle::ENGINE_FILE),
        b"not an engine",
    )
    .expect("corrupt the engine file");

    let evolved = compile(
        &GRAPH_SOURCE.replace("    subtitle: string\n", "    required subtitle: string\n"),
        GRAPH_IDS,
    );
    match attach(scratch.dir(), &evolved, projection_of(&evolved)) {
        Err(LifecycleError::Open(_)) => {}
        Err(other) => panic!(
            "a failing engine preempts the contract refusal, got code {}",
            other.code()
        ),
        Ok(_) => panic!("a garbage engine must not open"),
    }
}

/// The comparison itself, at its public seam: the pin a provision persists verifies against
/// the very image and projection it was derived from, and a permuted pin reports its first
/// disagreement in the derived walk order (deterministic, so the rendered refusal is a
/// stable function of the delta).
#[test]
fn verify_head_map_pin_accepts_the_provisioned_pin_and_orders_disagreements() {
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    let projection = projection_of(&image);
    let map = head_map(&image).expect("head map");
    assert_eq!(
        marrow_lifecycle::verify_head_map_pin(&image, &projection, &map),
        Ok(())
    );

    let mut ids: Vec<marrow_image::LedgerIdBytes> =
        map.entries().iter().map(|entry| entry.ledger_id).collect();
    ids.swap(1, 2);
    let permuted = marrow_lifecycle::HeadMap::assign(&ids).expect("assign");
    match marrow_lifecycle::verify_head_map_pin(&image, &projection, &permuted) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Binding {
                ledger_id: ids[2],
                persisted: Some(2),
                derived: Some(1),
            },
            "the first disagreement in derived walk order names the earlier node",
        ),
        Ok(()) => panic!("a permuted pin must disagree"),
    }
}

#[test]
fn adding_an_export_is_a_typed_interface_refusal() {
    let scratch = Scratch::new("iface");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    // A new pure export changes the exported interface, not the durable contract or ceiling.
    let extended = format!("{BASE_SOURCE}\npub fn two(): int {{\n    return 2\n}}\n");
    let changed = compile(&extended, BASE_IDS);

    let projection = projection_of(&changed);
    match attach(scratch.dir(), &changed, projection) {
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

    let projection = projection_of(&changed);
    match attach(scratch.dir(), &changed, projection) {
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

/// The contracted absence/tidy check over the open paths: plain `open` is the one public
/// `OpenStore` constructor that runs no head-map pin comparison, and its production caller
/// set is pinned here. Today that set is exactly the trusted bulk importer `import_jsonl`
/// (a WRITE path; the follow-on row threads the image into it so it can be fenced), and no
/// crate outside `marrow-lifecycle` names lifecycle `open` at all — every other production
/// route to a served store goes through `attach`, which runs the pin. A new caller turns up
/// in this listing and must either go through `attach` or extend the pin family.
#[test]
fn plain_open_has_exactly_the_documented_unfenced_callers() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = workspace.join("crates");
    assert!(
        crates.join("marrow-runner").is_dir() && crates.join("marrow-kernel").is_dir(),
        "workspace layout moved; rescope this scan"
    );

    let mut lifecycle_callers: Vec<(String, usize)> = Vec::new();
    let mut foreign_references: Vec<String> = Vec::new();
    let mut scanned = 0usize;
    let mut saw = (false, false); // (import.rs, provision.rs) — the scan reached its subjects
    for crate_dir in std::fs::read_dir(&crates).expect("list crates") {
        let crate_dir = crate_dir.expect("crate entry").path();
        let source_root = crate_dir.join("src");
        if !source_root.is_dir() {
            continue;
        }
        let in_lifecycle = crate_dir
            .file_name()
            .is_some_and(|name| name == "marrow-lifecycle");
        let mut stack = vec![source_root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("list sources") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs") {
                    continue;
                }
                // Production projection: comments, string/char literals, and #[cfg(test)]
                // items blanked, so a spelling inside prose, a literal, or a unit test can
                // neither trip nor silently satisfy the gate.
                let code = source_projection::production_code(
                    &std::fs::read_to_string(&path).expect("read source"),
                );
                scanned += 1;
                let name = path
                    .file_name()
                    .expect("a source file has a name")
                    .to_string_lossy()
                    .into_owned();
                if in_lifecycle {
                    saw.0 |= name == "import.rs";
                    saw.1 |= name == "provision.rs";
                    let calls = free_open_calls(&code);
                    if calls > 0 {
                        lifecycle_callers.push((name, calls));
                    }
                } else if names_lifecycle_open(&code) {
                    foreign_references.push(name);
                }
            }
        }
    }
    assert!(
        scanned > 100,
        "the scan visited too few files to be trusted ({scanned})"
    );
    assert!(
        saw.0 && saw.1,
        "the scan did not reach the open owner and its caller"
    );
    lifecycle_callers.sort();
    assert_eq!(
        lifecycle_callers,
        vec![("import.rs".to_string(), 1)],
        "plain `open` (no pin comparison) gained or lost a production caller",
    );
    assert_eq!(
        foreign_references,
        Vec::<String>::new(),
        "a crate outside marrow-lifecycle names lifecycle `open`, which would serve a store \
         without the head-map pin comparison",
    );
}

/// Count calls of lifecycle's plain `open(` in a literal-stripped projection: the bare
/// token (not `reopen(`, not a method call `.open(`, not `open_admitted(`, not the
/// `fn open(` definition, and not a foreign qualified call such as `File::open(`) plus the
/// crate's own qualified spelling `provision::open(`.
fn free_open_calls(code: &str) -> usize {
    let bytes = code.as_bytes();
    let mut count = 0;
    let mut from = 0;
    while let Some(found) = code[from..].find("open(") {
        let at = from + found;
        let bare = at
            .checked_sub(1)
            .map(|before| bytes[before])
            .is_none_or(|byte| {
                !source_projection::is_ident_byte(byte) && byte != b'.' && byte != b':'
            });
        let crate_qualified = code[..at].ends_with("provision::");
        let is_definition = code[..at].trim_end().ends_with("fn");
        if (bare || crate_qualified) && !is_definition {
            count += 1;
        }
        from = at + "open(".len();
    }
    count
}

/// Whether a foreign crate's projection names lifecycle `open`: a direct
/// `marrow_lifecycle::open` path at a token boundary, or a `use marrow_lifecycle::…;`
/// import whose item list carries the bare token `open`.
fn names_lifecycle_open(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut from = 0;
    while let Some(found) = code[from..].find("marrow_lifecycle::open") {
        let at = from + found;
        let end = at + "marrow_lifecycle::open".len();
        let before_ok = at
            .checked_sub(1)
            .map(|before| bytes[before])
            .is_none_or(|byte| !source_projection::is_ident_byte(byte));
        let after_ok = bytes
            .get(end)
            .is_none_or(|&byte| !source_projection::is_ident_byte(byte));
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    let mut from = 0;
    while let Some(found) = code[from..].find("use marrow_lifecycle::") {
        let start = from + found;
        let end = code[start..]
            .find(';')
            .map_or(code.len(), |semi| start + semi);
        if has_bare_open_token(&code[start..end]) {
            return true;
        }
        from = end;
    }
    false
}

/// Whether `text` carries the bare token `open` (exact, token-bounded).
fn has_bare_open_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(found) = text[from..].find("open") {
        let at = from + found;
        let end = at + "open".len();
        let before_ok = at
            .checked_sub(1)
            .map(|before| bytes[before])
            .is_none_or(|byte| !source_projection::is_ident_byte(byte));
        let after_ok = bytes
            .get(end)
            .is_none_or(|&byte| !source_projection::is_ident_byte(byte));
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// The plant probes for the caller scan: the scanner sees a real call and stays blind to
/// methods, other tokens, definitions, and spellings inside literals.
#[test]
fn the_open_caller_scanner_sees_calls_and_ignores_literals_and_methods() {
    assert_eq!(free_open_calls("let store = open(dir, projection);"), 1);
    assert_eq!(free_open_calls("open_admitted(dir, projection, admit)"), 0);
    assert_eq!(free_open_calls("engine.open(path)"), 0);
    assert_eq!(free_open_calls("reopen(dir)"), 0);
    assert_eq!(free_open_calls("File::open(path)"), 0);
    assert_eq!(
        free_open_calls("crate::provision::open(dir, projection)"),
        1
    );
    assert_eq!(free_open_calls("pub fn open(dir: &Path) {"), 0);
    assert_eq!(
        free_open_calls(&source_projection::production_code("let s = \"open(\";")),
        0,
        "a spelling inside a string literal is not a call",
    );
    assert!(names_lifecycle_open(
        "use marrow_lifecycle::{OpenError, open};"
    ));
    assert!(names_lifecycle_open(
        "marrow_lifecycle::open(dir, projection)"
    ));
    assert!(!names_lifecycle_open("use marrow_lifecycle::OpenStore;"));
    assert!(!names_lifecycle_open(&source_projection::production_code(
        "let s = \"marrow_lifecycle::open\";"
    )));
}
