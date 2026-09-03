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

/// The head-map pin bites (FR01 §3): a store is never attached under a numbering that
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

/// The image whose one group makes a kind swap numbering-neutral: `details` is the last
/// (only) group and there is no branch, so a projection that respells it as a keyed branch
/// numbers every node identically (root 0, title 1, details 2, pages 3) and matches every
/// path — only the node KIND differs.
const GROUPSWAP_SOURCE: &str = r#"resource Book {
    required title: string

    details {
        pages: int
    }
}

store ^books[id: int]: Book

pub fn readTitle(id: int): string {
    return ^books[id].title ?? "?"
}
"#;

const GROUPSWAP_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Book 0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d\n\
     id field Book.title 0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e0e\n\
     id group Book.details 20202020202020202020202020202020\n\
     id field Book.details.pages 21212121212121212121212121212121\n\
     id root books 0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b\n\
     id key books.id 0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c\n\
     high-water 0\n\
     end\n";

/// The pairing binds node KIND, not just names and numbers: a supplied projection that
/// respells the image's `details` group as a keyed branch — same paths, same dense
/// numbers, different physical layout — is refused at attach with a typed kind
/// disagreement. No production caller reaches this today (the runner derives its
/// projection from the same image it attaches), so this fence is hardening of the
/// attach seam, proven through the production entry itself.
#[test]
fn a_group_respelled_as_a_branch_projection_is_refused_by_kind() {
    use marrow_kernel::codec::value::ScalarKind;

    let scratch = Scratch::new("pin-kind-swap");
    let image = compile(GROUPSWAP_SOURCE, GROUPSWAP_IDS);
    provision_from(scratch.dir(), &image);

    // The substituted projection: `details` as a keyed branch instead of a group.
    let mut builder =
        marrow_kernel::durable::StoreSchemaBuilder::root("books", vec![ScalarKind::Int]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_branch("details", vec![ScalarKind::Int]);
    builder.scalar_field("pages", ScalarKind::Int, false);
    builder.close_branch();
    let schema = builder.finish().expect("the branch respelling builds");
    let mut projection = marrow_kernel::durable::StoreProjection::builder();
    projection.root(schema);
    let swapped = projection.finish().expect("the projection builds");

    match attach(scratch.dir(), &image, swapped) {
        Err(LifecycleError::HeadMapPin(refusal)) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Kind {
                place: "^books.details".to_string(),
                image: marrow_verify::SemanticNodeKind::Group,
                store: marrow_verify::SemanticNodeKind::Branch,
            },
        ),
        Err(other) => panic!("expected the kind refusal, got code {}", other.code()),
        Ok(_) => panic!(
            "a branch projection over group bytecode has a different physical layout and \
             must be refused, but attach served it"
        ),
    }
}

/// The pairing consumes every image-side durable node: a projection that under-covers the
/// image (here: the group and its field missing entirely) is refused as uncovered during
/// derivation itself — independent of the persisted map, so a correspondingly truncated
/// and resealed head cannot make the omission invisible. The persisted map handed in here
/// is exactly such a truncation, and the refusal still names the first uncovered node.
#[test]
fn a_projection_that_under_covers_the_image_is_refused_as_uncovered() {
    use marrow_kernel::codec::value::ScalarKind;

    let image = compile(GROUPSWAP_SOURCE, GROUPSWAP_IDS);
    let mut builder =
        marrow_kernel::durable::StoreSchemaBuilder::root("books", vec![ScalarKind::Int]);
    builder.scalar_field("title", ScalarKind::Str, true);
    let schema = builder.finish().expect("the truncated schema builds");
    let mut projection = marrow_kernel::durable::StoreProjection::builder();
    projection.root(schema);
    let truncated = projection.finish().expect("the projection builds");

    // A persisted map truncated to the same two nodes, validly assigned.
    let map = head_map(&image).expect("head map");
    let truncated_ids: Vec<marrow_image::LedgerIdBytes> = map
        .entries()
        .iter()
        .take(2)
        .map(|entry| entry.ledger_id)
        .collect();
    let truncated_map =
        marrow_lifecycle::HeadMap::assign(&truncated_ids).expect("a truncated map assigns");
    match marrow_lifecycle::verify_head_map_pin(&image, &truncated, &truncated_map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Uncovered {
                ledger_id: marrow_image::LedgerIdBytes::from_bytes([0x20; 16]),
                place: Some("^books.details".to_string()),
            },
        ),
        Ok(()) => panic!("an under-covering projection must refuse"),
    }
}

/// Two store roots of one resource: `^a.v` and `^b.v` are distinct durable nodes whose
/// ledger ids — the identity of the *declaration* `Entry.v` — are equal. A head map is a
/// bijection over ledger ids, so this program cannot be provisioned today, but the pin's
/// coverage check must still be injective over occurrences rather than over declarations.
const SHARED_PRODUCT_SOURCE: &str = r#"resource Entry {
    required v: int
}

store ^a[id: int]: Entry
store ^b[id: int]: Entry

pub fn readA(id: int): int {
    return ^a[id].v ?? 0
}
"#;

const SHARED_PRODUCT_IDS: &str = "marrow ids v0\n\
     machine-written by marrow; do not edit\n\
     id application . 0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a\n\
     id product Entry 50505050505050505050505050505050\n\
     id field Entry.v 51515151515151515151515151515151\n\
     id root a 52525252525252525252525252525252\n\
     id key a.id 53535353535353535353535353535353\n\
     id root b 54545454545454545454545454545454\n\
     id key b.id 55555555555555555555555555555555\n\
     high-water 0\n\
     end\n";

/// Coverage is decided over occurrence identity, not declaration identity. A projection
/// covering `^a` and its field but reaching `^b` without one leaves `^b.v` unaddressed;
/// because `^a.v` and `^b.v` share the ledger id of `Entry.v`, a check keyed on ledger ids
/// would count the first as covering the second and serve a store whose numbering omits a
/// node the program addresses. The persisted map handed in binds exactly the three nodes the
/// projection does reach, so nothing else can refuse it.
#[test]
fn coverage_is_decided_over_occurrence_identity_not_declaration_identity() {
    use marrow_kernel::codec::value::ScalarKind;

    let image = compile(SHARED_PRODUCT_SOURCE, SHARED_PRODUCT_IDS);
    let entry_v = marrow_image::LedgerIdBytes::from_bytes([0x51; 16]);

    let mut with_field =
        marrow_kernel::durable::StoreSchemaBuilder::root("a", vec![ScalarKind::Int]);
    with_field.scalar_field("v", ScalarKind::Int, true);
    let mut projection = marrow_kernel::durable::StoreProjection::builder();
    projection.root(with_field.finish().expect("the covered root builds"));
    projection.root(
        marrow_kernel::durable::StoreSchemaBuilder::root("b", vec![ScalarKind::Int])
            .finish()
            .expect("the fieldless root builds"),
    );
    let partial = projection.finish().expect("the projection builds");

    // Numbers 0, 1, 2 for `a`, `a.v`, `b` — the three nodes the projection reaches, validly
    // assigned, so the binding and high-water comparisons both agree.
    let map = marrow_lifecycle::HeadMap::assign(&[
        marrow_image::LedgerIdBytes::from_bytes([0x52; 16]),
        entry_v,
        marrow_image::LedgerIdBytes::from_bytes([0x54; 16]),
    ])
    .expect("the partial map assigns");

    match marrow_lifecycle::verify_head_map_pin(&image, &partial, &map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Uncovered {
                ledger_id: entry_v,
                place: Some("^b.v".to_string()),
            },
        ),
        Ok(()) => panic!("the second root's field is unreached and must refuse"),
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

/// The head-map pin pairs durable nodes by name and kind and says nothing about their
/// schema; the durable contract is the layer that binds the rest. `DurableContractId`'s
/// preimage carries each key column's scalar kind and the column count
/// (`marrow-image`'s `push_keys`) and each field's `required` flag and value shape
/// (`push_members`), so a recompiled program that changes any of them moves the contract and
/// is refused before the store is served. The boundary is recorded here so a later widening
/// or narrowing of either layer has evidence to move against; the fourth such fact, a
/// field's `required` flag, is pinned by `changing_the_durable_contract_is_a_typed_refusal`
/// above.
///
/// Every recompile below keeps every ledger id, every durable node name, and every node
/// kind, so the pin itself would pair and agree — only the contract moves. A refusal writes
/// nothing, so one provisioned store serves all three.
#[test]
fn a_changed_schema_fact_is_a_durable_contract_refusal() {
    let scratch = Scratch::new("schema-fact");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);

    let value_shape = BASE_SOURCE.replace("    label: string\n", "    label: int\n");
    // A key column's scalar kind, over the same key ledger id. The export's parameter type
    // follows the key; a resignature alone is not a binding-fact delta, so the refusal below
    // is the contract's, not the interface's.
    let key_scalar = BASE_SOURCE
        .replace("^counters[id: int]", "^counters[id: string]")
        .replace("readValue(n: int)", "readValue(n: string)");
    // A second key column, which moves both the count and the id set at once. The
    // isolated-count case is separate, below: arity IS separable from the ledger, in the
    // drop direction, and an earlier note here claimed it was not.
    let key_arity = BASE_SOURCE
        .replace("^counters[id: int]", "^counters[id: int, part: int]")
        .replace("readValue(n: int)", "readValue(n: int, p: int)")
        .replace("^counters[n]", "^counters[n, p]");
    let arity_ids = BASE_IDS.replace(
        "id key counters.id",
        "id key counters.part 5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c\nid key counters.id",
    );

    // The head as provisioned. A refusal must leave it byte-identical: the binding it
    // refused must not be the binding it stored. Without this the classification could
    // write the incoming binding on its way out and every assertion below would still
    // pass, so this is what makes the refusal a refusal rather than a report.
    let provisioned_head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head");

    for (fact, source, ids) in [
        ("a field's value shape", value_shape, BASE_IDS),
        ("a key column's scalar kind", key_scalar, BASE_IDS),
        ("a key tuple's arity", key_arity, arity_ids.as_str()),
    ] {
        let changed = compile(&source, ids);
        let projection = projection_of(&changed);
        match attach(scratch.dir(), &changed, projection) {
            Err(LifecycleError::ContractChanged(refusal)) => {
                assert_eq!(refusal.changed, ChangedFact::DurableContract, "{fact}");
                assert_eq!(refusal.code(), "store.contract_changed", "{fact}");
            }
            Err(other) => panic!(
                "{fact}: expected a durable-contract refusal, got code {}",
                other.code()
            ),
            Ok(_) => panic!("{fact} changed but the store was served"),
        }
        assert_eq!(
            std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head"),
            provisioned_head,
            "{fact}: the refusal rewrote the head it refused",
        );
    }
}

/// A key tuple's arity is a durable-contract fact in its own right, separable from the
/// ledger ids of its columns.
///
/// Round 4 recorded that it was NOT separable — that no case could change the arity while
/// preserving every column's ledger id — and skipped the pin on that ground. The claim is
/// false in the DROP direction: provision a two-column root, then attach a one-column one
/// against the identical ledger. Every ledger byte survives and the dropped column's id is
/// simply orphaned, so the ids are untouched and only the arity moved.
///
/// What makes the contract move is the shorter `(scalar, id)` run, not the `u16_be(count)`
/// that precedes it — removing that count leaves this case still refusing, because a
/// dropped column withdraws its own bytes from the preimage. Established by mutation
/// rather than by reading: the count looks like the mechanism and is not. The count is
/// pinned in its own right by the `durable_contract_id_*` known-answer tests beside
/// `push_keys`, which freeze the preimage byte for byte; this case pins the end-to-end
/// refusal, and the two together are why an arity change cannot be served.
#[test]
fn a_key_tuple_arity_change_alone_is_a_durable_contract_refusal() {
    let two_columns = BASE_SOURCE
        .replace("^counters[id: int]", "^counters[id: int, part: int]")
        .replace("readValue(n: int)", "readValue(n: int, p: int)")
        .replace("^counters[n]", "^counters[n, p]");
    // One ledger serves both shapes: the second column's id is present throughout, live in
    // the two-column image and orphaned in the one-column image.
    let ids = BASE_IDS.replace(
        "id key counters.id",
        "id key counters.part 5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c\n\
         id key counters.id",
    );

    let scratch = Scratch::new("arity-alone");
    let wide = compile(&two_columns, &ids);
    provision_from(scratch.dir(), &wide);
    let head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head");

    // The same ledger, one column narrower.
    let narrow = compile(BASE_SOURCE, &ids);
    match attach(scratch.dir(), &narrow, projection_of(&narrow)) {
        Err(LifecycleError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::DurableContract);
            assert_eq!(refusal.code(), "store.contract_changed");
        }
        Err(other) => panic!(
            "dropping a key column must be a durable-contract refusal, got code {}",
            other.code()
        ),
        Ok(_) => panic!("the key tuple narrowed but the store was served"),
    }
    assert_eq!(
        std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head"),
        head,
        "the refusal rewrote the head it refused",
    );
}

/// The absence/tidy check over the callers of lifecycle `open` — the one *spelling* of a
/// public `OpenStore` constructor that runs no head-map pin comparison.
///
/// What it detects: every production reference to that spelling, in any qualification the
/// classifier resolves (bare, `crate`/`self`/`super`/`provision`/`marrow_lifecycle`-
/// qualified, raw-identifier, turbofished). Its caller set is pinned to exactly one call,
/// inside the body of `import_jsonl` — the trusted bulk importer, a WRITE path the follow-on
/// row owns, and the same row owns removing this permitted call by threading the image in —
/// and no crate outside `marrow-lifecycle` may name lifecycle `open` at all. Every other
/// production route to an attached store goes through `attach`, which runs the pin, so a new
/// caller turns up here and must either go through `attach` or extend the pin family.
///
/// What it does not detect: a second unfenced public constructor that reaches an `OpenStore`
/// under another name. `open_admitted` is `pub(crate)` and takes an arbitrary admit closure,
/// so a public wrapper calling it with a no-op admit passes this scan unseen; so do a
/// re-export in a submodule (`mod raw { pub use crate::open; }`, called as `raw::open`), a
/// dependency rename declared in the *workspace* manifest and inherited as
/// `life.workspace = true` (only each crate's own `Cargo.toml` is read here), and an item a
/// macro emits from an argument the lexical projection erased — the tail sentinel compares
/// the same stripped source, so it does not catch that one either. Making those
/// unrepresentable is the follow-on row's, alongside the `import.rs` change; this check is a
/// caller census over one spelling, not a structural guarantee about the API.
///
/// The scan is lexical over the shared production projection (comments, string and char
/// literals, and `#[cfg(test)]` items blanked), resolving each `open` token by the tokens
/// around it into the closed set [`OpenReference`] enumerates. Rather than following an
/// alias, the gate refuses any `use` that would let a new spelling name the function — an
/// `as` alias of `open`, `provision`, or the crate's own path roots, a glob import of them —
/// and any dependency rename of `marrow-lifecycle` a crate's own manifest declares. Every
/// scanned file's last production item header must survive blanking at its own byte offset,
/// so a runaway blank cannot erase a call. Limitation: the shared projection carries no cfg
/// context — a test-only module included as `#[cfg(test)] mod name;` from its parent (only
/// `*_tests.rs` files are recognised as test-only) and an item under a compound marker such
/// as `#[cfg(all(test, unix))]` are scanned as production code; no such region names `open`
/// today, so nothing is falsely rejected.
#[test]
fn plain_open_has_exactly_the_documented_unfenced_callers() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = workspace.join("crates");
    assert!(
        crates.join("marrow-runner").is_dir() && crates.join("marrow-kernel").is_dir(),
        "workspace layout moved; rescope this scan"
    );

    let mut calls: Vec<(String, usize)> = Vec::new();
    let mut definitions: Vec<String> = Vec::new();
    let mut other_references: Vec<(String, usize)> = Vec::new();
    let mut alias_uses: Vec<(String, String)> = Vec::new();
    let mut foreign_references: Vec<(String, usize)> = Vec::new();
    let mut import_body: Option<std::ops::Range<usize>> = None;
    let mut scanned = 0usize;
    let mut saw = (false, false); // (import.rs, provision.rs) — the scan reached its subjects
    for crate_dir in std::fs::read_dir(&crates).expect("list crates") {
        let crate_dir = crate_dir.expect("crate entry").path();
        let source_root = crate_dir.join("src");
        if !source_root.is_dir() {
            continue;
        }
        let crate_name = crate_dir
            .file_name()
            .expect("a crate directory has a name")
            .to_string_lossy()
            .into_owned();
        let scope = if crate_name == "marrow-lifecycle" {
            Scope::Lifecycle
        } else {
            Scope::Foreign
        };
        if let Ok(manifest) = std::fs::read_to_string(crate_dir.join("Cargo.toml")) {
            assert!(
                !manifest
                    .lines()
                    .any(|line| line.contains("marrow-lifecycle") && line.contains("package")),
                "{crate_name} renames its marrow-lifecycle dependency, which this scan cannot \
                 follow",
            );
        }
        let mut stack = vec![source_root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("list sources") {
                let path = entry.expect("source entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "rs")
                    || source_projection::is_test_only_file(&path)
                {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("read source");
                let code = source_projection::production_code(&source);
                assert_projection_reaches_the_end(&path, &source, &code);
                scanned += 1;
                let name = path
                    .file_name()
                    .expect("a source file has a name")
                    .to_string_lossy()
                    .into_owned();
                let uses = use_statements(&code);
                let references = classify_open_references(&code, &uses, scope);
                let binds_open = references.iter().any(|reference| {
                    matches!(reference, OpenReference::Definition | OpenReference::Import)
                });
                for statement in &uses {
                    if introduces_alias(&code[statement.clone()], scope, binds_open) {
                        alias_uses.push((name.clone(), code[statement.clone()].to_string()));
                    }
                }
                for reference in references {
                    match reference {
                        OpenReference::Method | OpenReference::Foreign | OpenReference::Import => {}
                        OpenReference::Definition if scope == Scope::Lifecycle => {
                            definitions.push(name.clone());
                        }
                        OpenReference::Definition => {}
                        OpenReference::Call { at } if scope == Scope::Lifecycle => {
                            calls.push((name.clone(), at));
                        }
                        OpenReference::Other { at } if scope == Scope::Lifecycle => {
                            other_references.push((name.clone(), at));
                        }
                        OpenReference::Call { at } | OpenReference::Other { at } => {
                            foreign_references.push((name.clone(), at));
                        }
                    }
                }
                if scope == Scope::Lifecycle {
                    saw.0 |= name == "import.rs";
                    saw.1 |= name == "provision.rs";
                    if name == "import.rs" {
                        import_body = Some(function_body(&code, "import_jsonl"));
                    }
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
    assert_eq!(
        alias_uses,
        Vec::<(String, String)>::new(),
        "a `use` introduces a spelling of lifecycle `open` this scan does not follow",
    );
    assert_eq!(
        definitions,
        vec!["provision.rs".to_string()],
        "lifecycle `open` is defined exactly once, in provision.rs",
    );
    assert_eq!(
        other_references,
        Vec::<(String, usize)>::new(),
        "lifecycle `open` is named without being called (a function pointer, a re-export \
         alias, a parenthesised callee), which this scan refuses to follow",
    );
    let body = import_body.expect("import.rs was scanned");
    assert_eq!(
        calls.len(),
        1,
        "plain `open` (no pin comparison) gained or lost a production caller: {calls:?}",
    );
    let (file, at) = &calls[0];
    assert!(
        file == "import.rs" && body.contains(at),
        "the one permitted plain `open` call sits inside the body of `import_jsonl` \
         (bytes {body:?} of import.rs); found it at {file}:{at}",
    );
    assert_eq!(
        foreign_references,
        Vec::<(String, usize)>::new(),
        "a crate outside marrow-lifecycle names lifecycle `open`, which would serve a store \
         without the head-map pin comparison",
    );
}

/// Which crate a projection belongs to, which decides what an unqualified or
/// crate-relative `open` resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// Inside `marrow-lifecycle`: a bare `open`, or one qualified by `crate`, `self`,
    /// `super`, `provision`, or `marrow_lifecycle`, is the lifecycle function.
    Lifecycle,
    /// Any other crate: only a `marrow_lifecycle`-qualified `open` is the lifecycle
    /// function; a bare `open` is that crate's own unless a `use` imports lifecycle's
    /// (which the alias rule refuses).
    Foreign,
}

impl Scope {
    /// Whether a path qualifier immediately before `::open` names this crate's function.
    fn qualifies(self, qualifier: &str) -> bool {
        match self {
            Scope::Lifecycle => matches!(
                qualifier,
                "crate" | "self" | "super" | "provision" | "marrow_lifecycle"
            ),
            Scope::Foreign => qualifier == "marrow_lifecycle",
        }
    }
}

/// How one `open` token in a production projection resolves, decided by the tokens around
/// it. The set is closed: a token that is none of the foreign shapes is the lifecycle
/// function, and a lifecycle reference that is not a call is refused rather than followed.
#[derive(Debug, PartialEq, Eq)]
enum OpenReference {
    /// `.open` — a method or field, never this crate's function.
    Method,
    /// `<qualifier>::open` with a qualifier that does not name this crate's function
    /// (`File::open`, a foreign crate's own `open`), or a bare `open` outside the crate.
    Foreign,
    /// `fn open` — a definition.
    Definition,
    /// The token inside a `use` statement — the binding a call resolves through, which the
    /// alias rule inspects separately.
    Import,
    /// This crate's function, called: `open(`, `open (`, `open::<…>(`, in any of the
    /// qualifications [`Scope::qualifies`] admits with any spacing around `::`.
    Call { at: usize },
    /// This crate's function named without a call — a function pointer, a parenthesised
    /// callee `(open)(…)`, a re-export alias target.
    Other { at: usize },
}

/// Classify every `open` token in `code` (a production projection), given the byte spans
/// of its `use` statements.
fn classify_open_references(
    code: &str,
    uses: &[std::ops::Range<usize>],
    scope: Scope,
) -> Vec<OpenReference> {
    let mut references = Vec::new();
    for at in ident_token_offsets(code, "open") {
        let end = at + "open".len();
        if uses.iter().any(|span| span.contains(&at)) {
            references.push(OpenReference::Import);
            continue;
        }
        // A raw identifier is the same function under a different spelling, so step over the
        // `r#` prefix and let whatever qualifies it decide, exactly as for a bare `open`.
        let before = match token_before(code, at) {
            Some((hash, "#")) if hash + 1 == at => match token_before(code, hash) {
                Some((raw, "r")) if raw + 1 == hash => token_before(code, raw),
                _ => Some((hash, "#")),
            },
            other => other,
        };
        let lifecycle = match before {
            Some((_, ".")) => {
                references.push(OpenReference::Method);
                continue;
            }
            Some((_, "fn")) => {
                references.push(OpenReference::Definition);
                continue;
            }
            Some((colons, "::")) => match token_before(code, colons) {
                Some((_, qualifier)) => scope.qualifies(qualifier),
                None => scope == Scope::Lifecycle,
            },
            _ => scope == Scope::Lifecycle,
        };
        if !lifecycle {
            references.push(OpenReference::Foreign);
            continue;
        }
        let called = match token_after(code, end) {
            Some((_, "(")) => true,
            Some((colons, "::")) => matches!(token_after(code, colons + 2), Some((_, "<"))),
            _ => false,
        };
        references.push(if called {
            OpenReference::Call { at }
        } else {
            OpenReference::Other { at }
        });
    }
    references
}

/// The byte spans of every `use` and `extern crate` statement in `code`, each from its
/// keyword to its terminating `;`. `extern` opens a statement only in `extern crate`: as an
/// ABI marker (`extern "Rust" fn f() { … }`, or an `extern` block) it introduces an item
/// whose body is ordinary code, and a span running to the first `;` would swallow it.
///
/// A raw identifier is NOT the keyword it spells. `r#use` and `r#extern` are ordinary
/// names, so they open no statement — and the distinction is the opposite of the one the
/// call scan needs, which is why the two are handled in different places. `open` is an
/// identifier, so `r#open` names the same function and the call scan steps over the
/// prefix to see it. `use` is a keyword, so `r#use` names something else entirely and
/// this scan must not see it. Reading a raw identifier as the keyword opens a span to the
/// next `;` over code that is not an import, and a call inside that span disappears:
/// `#[cfg_attr(any(), r#use)] let leaked = r#open(dir, projection);` hid its call from
/// every assertion here.
fn use_statements(code: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    for keyword in ["use", "extern"] {
        for at in ident_token_offsets(code, keyword) {
            if code[..at].ends_with("r#") {
                continue;
            }
            if keyword == "extern"
                && !matches!(token_after(code, at + keyword.len()), Some((_, "crate")))
            {
                continue;
            }
            let end = code[at..]
                .find(';')
                .map_or(code.len(), |semi| at + semi + 1);
            spans.push(at..end);
        }
    }
    spans
}

/// Whether one `use` statement introduces a spelling of lifecycle `open` this scan does
/// not follow: an `as` alias whose subject is `open`, `provision`, `marrow_lifecycle`, or —
/// inside the crate, in a statement rooted at its own module tree — `crate`, `self`, or
/// `super`; a glob import from `provision`, `marrow_lifecycle`, or the crate root (which
/// re-exports `open`); or, when the enclosing file itself binds `open` (`binds_open`: it
/// defines or imports it, so a child module's `super::*` reaches it), any glob rooted at
/// the file's own module tree.
fn introduces_alias(statement: &str, scope: Scope, binds_open: bool) -> bool {
    let rooted = ["crate", "self", "super"].iter().any(|root| {
        ident_token_offsets(statement, root)
            .any(|at| matches!(token_after(statement, at + root.len()), Some((_, "::"))))
    });
    let mentions_lifecycle = has_ident_token(statement, "marrow_lifecycle");
    if scope == Scope::Foreign {
        return mentions_lifecycle
            && (has_ident_token(statement, "open")
                || statement.contains('*')
                || ident_token_offsets(statement, "as").any(|at| {
                    matches!(
                        token_before(statement, at),
                        Some((_, "marrow_lifecycle" | "self" | "open"))
                    )
                }));
    }
    let aliases = ident_token_offsets(statement, "as").any(|at| {
        matches!(token_before(statement, at), Some((_, subject))
            if matches!(subject, "open" | "provision" | "marrow_lifecycle")
                || (rooted && matches!(subject, "crate" | "self" | "super")))
    });
    let globs = statement.match_indices('*').any(|(at, _)| {
        let subject = match token_before(statement, at) {
            Some((colons, "::")) => token_before(statement, colons).map(|(_, subject)| subject),
            _ => None,
        };
        matches!(subject, Some("provision" | "marrow_lifecycle" | "crate"))
            || (rooted && binds_open)
    });
    aliases || globs
}

/// The byte span of the body of `fn <name>` in `code`: from its opening brace to just past
/// its closing brace. Exactly one such definition must exist.
fn function_body(code: &str, name: &str) -> std::ops::Range<usize> {
    let mut definitions = ident_token_offsets(code, name)
        .filter(|&at| matches!(token_before(code, at), Some((_, "fn"))));
    let at = definitions.next().expect("the function is defined");
    assert!(
        definitions.next().is_none(),
        "`fn {name}` is defined more than once"
    );
    let open = at + code[at..].find('{').expect("the function has a body");
    let mut depth = 0usize;
    for (offset, byte) in code.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return open..offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!("the body of `fn {name}` is unterminated")
}

/// The projection reaches the END of `path`: its last production item header — or, for a
/// file with none (a module list), its last production line free of literal and comment
/// text — survives blanking at the byte offset it has in the source with only the test
/// items removed. The sentinel is derived per file, so every scanned file is covered
/// without a list to maintain, and a file offering no sentinel at all fails loudly.
fn assert_projection_reaches_the_end(path: &Path, source: &str, code: &str) {
    let production = source_projection::without_cfg_test_items(source);
    let sentinel = source_projection::last_production_item(source)
        .or_else(|| {
            production
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| {
                    !line.is_empty()
                        && !line.contains('"')
                        && !line.contains('\'')
                        && !line.contains("//")
                        && !line.contains("/*")
                })
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            panic!(
                "{} offers no tail sentinel this gate can follow to its end",
                path.display()
            )
        });
    assert_eq!(
        code.rfind(&sentinel),
        production.rfind(&sentinel),
        "the projection lost or moved the tail of {}: `{sentinel}`",
        path.display()
    );
}

/// The byte offsets of every occurrence of the identifier `needle` in `text` as a whole
/// token (not part of a longer identifier).
fn ident_token_offsets<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    let bytes = text.as_bytes();
    text.match_indices(needle).filter_map(move |(at, _)| {
        let end = at + needle.len();
        let before_ok = at
            .checked_sub(1)
            .is_none_or(|before| !source_projection::is_ident_byte(bytes[before]));
        let after_ok = bytes
            .get(end)
            .is_none_or(|&byte| !source_projection::is_ident_byte(byte));
        (before_ok && after_ok).then_some(at)
    })
}

fn has_ident_token(text: &str, needle: &str) -> bool {
    ident_token_offsets(text, needle).next().is_some()
}

/// The token ending just before byte `end` of `text`, skipping whitespace: an identifier,
/// `::`, or one other byte, with its start offset.
fn token_before(text: &str, end: usize) -> Option<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut end = end;
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let start = if source_projection::is_ident_byte(bytes[end - 1]) {
        let mut start = end;
        while start > 0 && source_projection::is_ident_byte(bytes[start - 1]) {
            start -= 1;
        }
        start
    } else if bytes[..end].ends_with(b"::") {
        end - 2
    } else {
        end - 1
    };
    Some((start, text.get(start..end).unwrap_or("\u{fffd}")))
}

/// The token starting at or after byte `start` of `text`, skipping whitespace: an
/// identifier, `::`, or one other byte, with its start offset.
fn token_after(text: &str, start: usize) -> Option<(usize, &str)> {
    let bytes = text.as_bytes();
    let mut start = start;
    while start < bytes.len() && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    if start == bytes.len() {
        return None;
    }
    let end = if source_projection::is_ident_byte(bytes[start]) {
        let mut end = start;
        while end < bytes.len() && source_projection::is_ident_byte(bytes[end]) {
            end += 1;
        }
        end
    } else if bytes[start..].starts_with(b"::") {
        start + 2
    } else {
        start + 1
    };
    Some((start, text.get(start..end).unwrap_or("\u{fffd}")))
}

/// The plant probes for the caller scan: every spelling that can legally name the lifecycle
/// function is either classified as a call or refused as an alias, and every foreign shape
/// stays invisible.
#[test]
fn the_open_caller_scanner_resolves_every_spelling_and_ignores_foreign_shapes() {
    use OpenReference::{Call, Definition, Foreign, Import, Method, Other};
    let classify = |code: &str, scope: Scope| {
        let code = source_projection::production_code(code);
        classify_open_references(&code, &use_statements(&code), scope)
    };
    let lifecycle = |code: &str| classify(code, Scope::Lifecycle);

    assert_eq!(
        lifecycle("let store = open(dir, projection);"),
        [Call { at: 12 }]
    );
    assert_eq!(lifecycle("crate::open(dir, projection)"), [Call { at: 7 }]);
    assert_eq!(
        lifecycle("crate::provision::open(dir, p)"),
        [Call { at: 18 }]
    );
    assert_eq!(
        lifecycle("marrow_lifecycle :: open(dir, p)"),
        [Call { at: 20 }]
    );
    assert_eq!(lifecycle("open (dir, projection)"), [Call { at: 0 }]);
    assert_eq!(lifecycle("open::<Store>(dir)"), [Call { at: 0 }]);
    assert_eq!(lifecycle("(open)(dir, projection)"), [Other { at: 1 }]);
    assert_eq!(lifecycle("let f = open;"), [Other { at: 8 }]);
    assert_eq!(lifecycle("pub fn open(dir: &Path) {"), [Definition]);
    assert_eq!(lifecycle("engine.open(path)"), [Method]);
    assert_eq!(lifecycle("File::open(path)"), [Foreign]);
    assert_eq!(
        lifecycle("use crate::provision::{OpenError, open};"),
        [Import]
    );
    assert!(lifecycle("open_admitted(dir, p, admit); reopen(dir)").is_empty());
    // A raw identifier names the same function, in every qualification.
    assert_eq!(lifecycle("r#open(dir, projection)"), [Call { at: 2 }]);
    assert_eq!(lifecycle("crate::r#open(dir, p)"), [Call { at: 9 }]);
    // And a raw identifier spelling a KEYWORD is not that keyword, so it opens no import
    // span. Read as `use`, the span below runs to the `;` and the call inside it vanishes.
    assert_eq!(
        lifecycle("#[cfg_attr(any(), r#use)] let leaked = r#open(dir, projection);"),
        [Call { at: 41 }]
    );
    assert_eq!(
        lifecycle("#[cfg_attr(any(), r#extern)] let s = open(dir, p);"),
        [Call { at: 37 }]
    );
    assert!(use_statements("#[cfg_attr(any(), r#use)] let x = 1;").is_empty());
    assert!(use_statements("#[cfg_attr(any(), r#extern)] let x = 1;").is_empty());
    // The keyword itself still opens one, so the exclusion is about the prefix only.
    assert_eq!(use_statements("use crate::provision::open;").len(), 1);
    assert_eq!(use_statements("extern crate marrow_kernel;").len(), 1);
    // An ABI `extern` marks an item, not an import: its body stays visible to the scan.
    assert_eq!(
        lifecycle(
            "pub extern \"Rust\" fn resume(d: &Path) -> R {\n  let r = open(d, p);\n  r\n}\n"
        ),
        [Call { at: 55 }]
    );
    assert!(
        lifecycle("let s = \"open(\"; // open(\n").is_empty(),
        "literals are not code"
    );
    assert!(lifecycle("#[cfg(test)]\nmod t {\n fn f() { open(d, p); }\n}\n").is_empty());

    assert_eq!(classify("open(dir, projection)", Scope::Foreign), [Foreign]);
    assert_eq!(
        classify("marrow_lifecycle::open(dir, p)", Scope::Foreign),
        [Call { at: 18 }]
    );
    assert_eq!(classify("crate::open(dir)", Scope::Foreign), [Foreign]);
    assert_eq!(
        classify("marrow_lifecycle::r#open(dir, p)", Scope::Foreign),
        [Call { at: 20 }]
    );

    for alias in [
        "use crate::open as raw_open;",
        "use crate::provision as p;",
        "use crate::provision::{self as p};",
        "use crate::provision::*;",
        "use crate::*;",
        "use marrow_lifecycle as life;",
        "use marrow_lifecycle::*;",
        "extern crate marrow_lifecycle as life;",
    ] {
        assert!(introduces_alias(alias, Scope::Lifecycle, false), "{alias}");
    }
    // A glob rooted at the file's own module tree reaches `open` exactly when the file
    // binds it.
    assert!(introduces_alias("use super::*;", Scope::Lifecycle, true));
    assert!(introduces_alias(
        "use self::helpers::*;",
        Scope::Lifecycle,
        true
    ));
    for plain in [
        "use crate::provision::{OpenError, open};",
        "use std::io::{self as io};",
        "use marrow_lifecycle::OpenStore;",
        "use super::*;",
    ] {
        assert!(!introduces_alias(plain, Scope::Lifecycle, false), "{plain}");
    }
    for foreign_alias in [
        "use marrow_lifecycle::{OpenError, open};",
        "use marrow_lifecycle as life;",
        "use marrow_lifecycle::*;",
        "use marrow_lifecycle::{self as life};",
    ] {
        assert!(
            introduces_alias(foreign_alias, Scope::Foreign, true),
            "{foreign_alias}"
        );
    }
    for foreign_plain in [
        "use marrow_lifecycle::OpenStore;",
        "use crate::provision as p;",
        "use self::builtins::*;",
        "use super::*;",
    ] {
        assert!(
            !introduces_alias(foreign_plain, Scope::Foreign, true),
            "{foreign_plain}"
        );
    }

    let sample = "fn a() { }\npub fn import_jsonl(d: &Path) -> R {\n  open(d)\n}\n";
    let body = function_body(sample, "import_jsonl");
    assert_eq!(
        body,
        sample.find("{\n").expect("body opens")..sample.rfind('}').expect("body closes") + 1
    );
    assert!(body.contains(&sample.find("open(").expect("the call")));
}
