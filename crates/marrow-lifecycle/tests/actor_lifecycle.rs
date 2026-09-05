//! The lifecycle actor over a real compiled durable image: binding-facts derivation, the
//! head-map ↔ kernel-numbering agreement, and the attach classifier (already-active, the
//! binding-only rebind, and the typed contract-changed refusals).

use std::path::{Path, PathBuf};

use marrow_lifecycle::{
    AttachOutcome, ChangedFact, EngineKind, HEAD_FILE, LifecycleError, LogicalHead,
    PinDisagreement, ProvisionRequest, StoreEnvelope, StoreInstanceId, active_binding, attach,
    head_map, prepare, provision,
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
    compile_files(&[("src/main.mw", source)], ids)
}

/// Compile a project of several modules, so an export's *module* — half of its declaration
/// path identity — can be varied as well as its item name.
fn compile_files(sources: &[(&str, &str)], ids: &str) -> VerifiedImage {
    let manifest = marrow_project::Manifest::parse("edition = \"2026\"\n").expect("manifest");
    let files = sources
        .iter()
        .map(|(path, text)| {
            marrow_project::CapturedFile::new(path.to_string(), text.as_bytes().to_vec())
        })
        .collect();
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

/// The store projection the lifecycle derives for `image`, for inspection.
fn projection_of(image: &VerifiedImage) -> marrow_kernel::durable::StoreProjection {
    prepare(image.clone())
        .projection()
        .cloned()
        .expect("the base image is flat-executable")
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
    provision(dir, ProvisionRequest { envelope, head }).expect("provision");
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

/// A pure export, declared apart from the durable half so it can be renamed, relocated to
/// another module, and resignatured without disturbing anything else.
const PURE_EXPORT: &str = "pub fn two(): int {\n    return 2\n}\n";

/// The interface fingerprint a store persists moves exactly with the export set, measured
/// through the production projection — `active_binding` over really compiled images —
/// rather than over hand-minted ids: an export added, removed, renamed, or relocated to
/// another module moves it, while reordering the declarations and *resignaturing* an export
/// leave it standing.
///
/// The two stillnesses carry the invariant-A claim that the slot is an export-SET identity,
/// blind to signatures, so each is taken against an image that really differs — asserted
/// here, because a stillness compared against a repeated derivation of one image would hold
/// no matter what the fingerprint digested.
#[test]
fn the_persisted_interface_fingerprint_moves_exactly_with_the_export_set() {
    let facts = |image: &VerifiedImage| {
        let binding = active_binding(image);
        (binding.image_id, binding.interface)
    };
    let base = format!("{BASE_SOURCE}\n{PURE_EXPORT}");
    let baseline = facts(&compile(&base, BASE_IDS));

    let added = format!("{base}\npub fn three(): int {{\n    return 3\n}}\n");
    let renamed = base.replace("fn two()", "fn deux()");
    let relocated = format!("module extra\n\n{PURE_EXPORT}");
    for (movement, image) in [
        ("an export added", compile(&added, BASE_IDS)),
        ("an export removed", compile(BASE_SOURCE, BASE_IDS)),
        ("an export renamed", compile(&renamed, BASE_IDS)),
        (
            "an export relocated",
            compile_files(
                &[("src/main.mw", BASE_SOURCE), ("src/extra.mw", &relocated)],
                BASE_IDS,
            ),
        ),
    ] {
        assert_ne!(
            baseline.1,
            facts(&image).1,
            "{movement} must move the persisted fingerprint",
        );
    }

    let (head, read) = BASE_SOURCE
        .split_once("pub fn readValue")
        .expect("the base declares its export");
    let reordered = format!("{head}{PURE_EXPORT}\npub fn readValue{read}");
    let resignatured = base.replace("fn two(): int", "fn two(k: int): int");
    for (stillness, image) in [
        ("the declarations reordered", compile(&reordered, BASE_IDS)),
        ("an export resignatured", compile(&resignatured, BASE_IDS)),
    ] {
        let changed = facts(&image);
        assert_ne!(
            baseline.0, changed.0,
            "{stillness} must really change the image, or the stillness proves nothing",
        );
        assert_eq!(
            baseline.1, changed.1,
            "{stillness} must not move the persisted fingerprint",
        );
    }
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

    match attach(scratch.dir(), prepare(image)).expect("attach") {
        AttachOutcome::AlreadyActive(attachment) => drop(attachment),
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

    let receipt = match attach(scratch.dir(), prepare(edited.clone())).expect("attach") {
        AttachOutcome::Rebound {
            attachment,
            receipt,
        } => {
            drop(attachment);
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

/// Reopen the store under `image` (its active binding), returning the persisted logical head.
fn open_head(dir: &Path, image: &VerifiedImage) -> LogicalHead {
    match attach(dir, prepare(image.clone())).expect("attach") {
        AttachOutcome::AlreadyActive(attachment) => attachment.head().clone(),
        AttachOutcome::Rebound { .. } => panic!("the active image is already active"),
    }
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

    // Simulate the crash: read the persisted head, stamp the new binding into it (the commit
    // point), and write only the head back — leaving the old envelope, exactly the on-disk
    // state a kill between the head rename and the envelope rewrite leaves.
    let crashed_head = LogicalHead {
        binding: binding_b,
        ..open_head(scratch.dir(), &image_a)
    }
    .encode();
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

    match attach(scratch.dir(), prepare(image)) {
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

    match attach(scratch.dir(), prepare(image)) {
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
    match attach(scratch.dir(), prepare(edited.clone())) {
        Err(LifecycleError::HeadMapPin(_)) => {}
        Err(other) => panic!("expected the pin refusal, got code {}", other.code()),
        Ok(_) => panic!("a rebind over a permuted pin was served"),
    }
    assert_eq!(
        std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head"),
        before_head,
        "the refusal rewrote the head it refused",
    );
    assert_eq!(
        std::fs::read(scratch.dir().join(marrow_lifecycle::ENVELOPE_FILE)).expect("read envelope"),
        before_envelope,
        "the refusal rewrote the envelope",
    );

    // The refusal released the lock: with the true pin restored, the same rebind commits.
    std::fs::write(scratch.dir().join(HEAD_FILE), &true_head).expect("restore the true head");
    match attach(scratch.dir(), prepare(edited)).expect("attach") {
        AttachOutcome::Rebound { attachment, .. } => drop(attachment),
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
    match attach(scratch.dir(), prepare(evolved)) {
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

    match attach(scratch.dir(), prepare(image)) {
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
/// number) pairs, and the pin refuses. Kernel numbers are dense over any projection shape,
/// so only the name pairing — not walk position — catches this. No public route pairs a
/// caller's projection with a store, so the drift is checked at the pin owner against the
/// map a provision persists.
#[test]
fn a_drifted_projection_derivation_is_refused_by_the_pin() {
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);

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
    match marrow_lifecycle::verify_head_map_pin(&image, &drifted, &map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Binding {
                ledger_id: tags_root,
                persisted: Some(pinned_tags),
                derived: Some(0),
            },
        ),
        Ok(()) => panic!("a drifted projection derivation readdresses cells and must refuse"),
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

    let image = compile(GROUPSWAP_SOURCE, GROUPSWAP_IDS);

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

    let map = head_map(&image).expect("head map");
    match marrow_lifecycle::verify_head_map_pin(&image, &swapped, &map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Kind {
                place: "^books.details".to_string(),
                image: marrow_verify::SemanticNodeKind::Group,
                store: marrow_verify::SemanticNodeKind::Branch,
            },
        ),
        Ok(()) => panic!(
            "a branch projection over group bytecode has a different physical layout and \
             must be refused"
        ),
    }
}

/// The pairing consumes every image-side durable node the walk numbers — every semantic node but
/// a managed `Index`, which is neither named nor claimed because its cell keys carry an identity
/// rather than a number. A projection that under-covers the
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

/// Two store roots of one resource, so every member of `Entry` is two durable nodes under
/// one ledger id — `^a.v` and `^b.v` are the identity of the *declaration* `Entry.v`, and so
/// are `^a.meta` and `^b.meta`, `^a.notes` and `^b.notes`, and the nested `replies` under
/// each. A head map is a bijection over ledger ids, so this program cannot be provisioned
/// today, but the pin's coverage check must still be injective over occurrences rather than
/// over declarations, at every node kind and at every depth.
const SHARED_PRODUCT_SOURCE: &str = r#"resource Entry {
    required v: int

    meta {
        m: int
    }

    notes[noteId: string] {
        required body: string

        replies[replyId: string] {
            required text: string
        }
    }
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
     id group Entry.meta 56565656565656565656565656565656\n\
     id field Entry.meta.m 57575757575757575757575757575757\n\
     id root Entry.notes 58585858585858585858585858585858\n\
     id key Entry.notes.noteId 59595959595959595959595959595959\n\
     id field Entry.notes.body 5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a\n\
     id root Entry.notes.replies 5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b5b\n\
     id key Entry.notes.replies.replyId 5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c\n\
     id field Entry.notes.replies.text 5d5d5d5d5d5d5d5d5d5d5d5d5d5d5d5d\n\
     high-water 0\n\
     end\n";

/// One `Entry` root of a coverage case's store shape, carrying the named parts of the shared
/// resource: `v` (the flat field), `meta` (the group and its field), `notes` (the keyed
/// branch and its field), `replies` (the sub-branch nested inside `notes`).
fn entry_root(name: &str, parts: &str) -> marrow_kernel::durable::StoreSchema {
    use marrow_kernel::codec::value::ScalarKind;

    let has = |part: &str| parts.split_whitespace().any(|token| token == part);
    let mut builder = marrow_kernel::durable::StoreSchemaBuilder::root(name, vec![ScalarKind::Int]);
    if has("v") {
        builder.scalar_field("v", ScalarKind::Int, true);
    }
    if has("meta") {
        builder.open_group("meta");
        builder.scalar_field("m", ScalarKind::Int, false);
        builder.close_group();
    }
    if has("notes") {
        builder.open_branch("notes", vec![ScalarKind::Str]);
        builder.scalar_field("body", ScalarKind::Str, true);
        if has("replies") {
            builder.open_branch("replies", vec![ScalarKind::Str]);
            builder.scalar_field("text", ScalarKind::Str, true);
            builder.close_branch();
        }
        builder.close_branch();
    }
    builder.finish().expect("the root builds")
}

/// Coverage is decided over occurrence identity, not declaration identity — at every node
/// kind and at every depth. In the flat-field, keyed-branch and group cases the omitted
/// occurrence's ledger id is present in the walk under the *other* root, so a check keyed on
/// ledger ids would count that one as covering this one and serve a store whose numbering
/// omits a node the program addresses. A group and a keyed branch therefore carry the
/// property in their own right; pinning it on the flat field alone leaves a coverage check
/// that consults ids for the rest. The nested-branch case is a DEPTH case, not an identity
/// one: it projects `replies` under neither root, so what it establishes is that the walk
/// reaches a branch member nested inside a branch at all. Isolating a nested occurrence
/// against a live twin, and the fields below a group or a branch, belongs to the follow-on
/// row.
///
/// The persisted map binds only the two store roots. Coverage is decided during derivation,
/// before the persisted map is read, so what the map binds cannot make an uncovered node
/// covered — and were the map what refused, the exact typed disagreement asserted here would
/// fail rather than pass.
#[test]
fn coverage_is_decided_over_occurrence_identity_not_declaration_identity() {
    let image = compile(SHARED_PRODUCT_SOURCE, SHARED_PRODUCT_IDS);
    let map = marrow_lifecycle::HeadMap::assign(&[
        marrow_image::LedgerIdBytes::from_bytes([0x52; 16]),
        marrow_image::LedgerIdBytes::from_bytes([0x54; 16]),
    ])
    .expect("the root-only map assigns");

    // Each case gives `^a` the parts left of the bar and `^b` those right of it.
    for (kind, split, ledger, place) in [
        ("a flat field", "v meta notes replies|", 0x51, "^b.v"),
        ("a keyed branch", "v meta|notes replies", 0x58, "^a.notes"),
        ("a nested branch", "v meta notes|", 0x5b, "^a.notes.replies"),
        ("a group", "v notes replies|meta", 0x56, "^a.meta"),
    ] {
        let (a, b) = split.split_once('|').expect("both roots' parts");
        let mut projection = marrow_kernel::durable::StoreProjection::builder();
        projection.root(entry_root("a", a));
        projection.root(entry_root("b", b));
        let partial = projection.finish().expect("the projection builds");
        match marrow_lifecycle::verify_head_map_pin(&image, &partial, &map) {
            Err(refusal) => assert_eq!(
                refusal.disagreement,
                PinDisagreement::Uncovered {
                    ledger_id: marrow_image::LedgerIdBytes::from_bytes([ledger; 16]),
                    place: Some(place.to_string()),
                },
                "{kind}: the first unreached occurrence",
            ),
            Ok(()) => panic!("{kind}: an unreached occurrence must refuse"),
        }
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
    match attach(scratch.dir(), prepare(evolved)) {
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

    match attach(scratch.dir(), prepare(changed)) {
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

    match attach(scratch.dir(), prepare(changed)) {
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
/// or narrowing of either layer has evidence to move against.
///
/// Every fact below is changed in the program's **second** store root. The graph payload
/// walks the roots and writes each one's own keys and members; a walk that wrote the first
/// root's for every root would carry the same identity under all four of these changes, and
/// a one-root fixture cannot tell the two walks apart.
///
/// Every recompile keeps every ledger id, every durable node name, and every node kind, so the
/// pin itself would pair and agree — only the contract moves. What the loop below reads back is
/// the head, and a refusal leaves those bytes exactly as provisioned, which is why one provisioned
/// store serves every case. The store's owner marker is outside that: taking the lock binds this
/// process into it and releasing the lock truncates it, on a refusal as on a success.
#[test]
fn a_changed_schema_fact_is_a_durable_contract_refusal() {
    let scratch = Scratch::new("schema-fact");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);

    let value_shape = GRAPH_SOURCE.replace("required name: string", "required name: int");
    let required = GRAPH_SOURCE.replace("required name: string", "name: string");
    let key_scalar = GRAPH_SOURCE.replace("^tags[id: int]", "^tags[id: string]");
    // A second key column, which moves both the count and the id set at once. The isolated
    // count case is separate, below: arity is separable from the ledger in the drop direction.
    let key_arity = GRAPH_SOURCE.replace("^tags[id: int]", "^tags[id: int, part: int]");
    let arity_ids = GRAPH_IDS.replace(
        "id key tags.id",
        "id key tags.part 5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c5c\nid key tags.id",
    );

    // The head as provisioned. A refusal must leave it byte-identical: the binding it
    // refused must not be the binding it stored. Without this the classification could
    // write the incoming binding on its way out and every assertion below would still
    // pass, so this is what makes the refusal a refusal rather than a report.
    let provisioned_head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head");

    for (fact, source, ids) in [
        ("a field's value shape", value_shape, GRAPH_IDS),
        ("a field's required flag", required, GRAPH_IDS),
        ("a key column's scalar kind", key_scalar, GRAPH_IDS),
        ("a key tuple's arity", key_arity, arity_ids.as_str()),
    ] {
        let changed = compile(&source, ids);
        match attach(scratch.dir(), prepare(changed)) {
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
/// Separability holds in the DROP direction: provision a two-column root, then attach a
/// one-column one against the identical ledger. Every ledger byte survives and the dropped
/// column's id is simply orphaned, so the ids are untouched and only the arity moved.
///
/// What makes the contract move is the shorter `(scalar, id)` run, not the `u16_be(count)` that
/// precedes it — removing that count leaves this case still refusing, because a dropped column
/// withdraws its own bytes from the preimage. Established by mutation rather than by reading: the
/// count looks like the mechanism and is not. The count is pinned in its own right by the
/// `durable_contract_id_*` known-answer tests beside `push_keys`, which freeze the preimage byte
/// for byte; this case pins the end-to-end refusal, and the two together are why an arity change
/// cannot be served.
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
    match attach(scratch.dir(), prepare(narrow)) {
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
