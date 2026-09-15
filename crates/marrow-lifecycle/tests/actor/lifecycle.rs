//! The lifecycle actor over a real compiled durable image: binding-facts derivation, the
//! head-map ↔ kernel-numbering agreement, and the attach classifier (already-active, the
//! binding-only rebind, and the typed contract-changed refusals).

use std::path::Path;

use crate::support::actor_fixtures::*;
use crate::support::compile::compile_files;
use marrow_lifecycle::{
    AttachOutcome, ChangedFact, HEAD_FILE, LifecycleError, LogicalHead, PinDisagreement,
    active_binding, attach, head_map, prepare,
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
    verify(&compile_files(&[("src/main.mw", source)], ids)).expect("verify")
}

/// The store projection the lifecycle derives for `image`, for inspection.
fn projection_of(image: &VerifiedImage) -> marrow_kernel::durable::StoreProjection {
    prepare(image.clone())
        .projection()
        .cloned()
        .expect("the base image is flat-executable")
}

use marrow_codes::Code;

use crate::support::Scratch;
use crate::support::store::provision_from;

#[test]
fn refused_attach_preserves_absent_owner_marker() {
    refused_attach_preserves_marker(None);
}

#[test]
fn refused_attach_preserves_nonempty_owner_marker() {
    refused_attach_preserves_marker(Some(b"inherited obligation"));
}

fn refused_attach_preserves_marker(marker: Option<&[u8]>) {
    let scratch = Scratch::new("refusal-marker");
    let image = compile(BASE_SOURCE, BASE_IDS);
    provision_from(scratch.dir(), &image);
    assert!(!scratch.dir().join("lock").exists());
    if let Some(marker) = marker {
        std::fs::write(scratch.dir().join("lock"), marker).expect("seed marker");
    }
    let mut before = std::fs::read_dir(scratch.dir())
        .expect("list provisioned store")
        .map(|entry| {
            let entry = entry.expect("store entry");
            let bytes = std::fs::read(entry.path()).expect("read store artifact");
            (entry.file_name(), bytes)
        })
        .collect::<Vec<_>>();
    before.sort_by(|left, right| left.0.cmp(&right.0));

    let changed = compile(
        &BASE_SOURCE.replace("    label: string\n", "    required label: string\n"),
        BASE_IDS,
    );
    match attach(scratch.dir(), prepare(changed)) {
        Err(LifecycleError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::DurableContract);
        }
        Err(error) => panic!("expected contract refusal, got {}", error.code().as_str()),
        Ok(_) => panic!("an incompatible image was attached"),
    }
    for (name, bytes) in &before {
        assert!(
            std::fs::read(scratch.dir().join(name)).expect("read refused store") == *bytes,
            "admission changed {name:?}",
        );
    }
    let mut after = std::fs::read_dir(scratch.dir())
        .expect("list refused store")
        .map(|entry| entry.expect("store entry").file_name())
        .collect::<Vec<_>>();
    after.sort();
    let names = before.into_iter().map(|(name, _)| name).collect::<Vec<_>>();
    assert_eq!(after, names, "refused admission changed store membership");
}

#[test]
fn explicit_recovery_preserves_populated_program_data() {
    use marrow_vm::{DurableRun, Value, run_export};
    let source = format!(
        "{BASE_SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
    );
    let image = compile(&source, BASE_IDS);
    let export = |name: &str| {
        image
            .exports()
            .iter()
            .find(|export| {
                image
                    .function(export.function())
                    .expect("verified function")
                    .body()
                    .name()
                    == name
            })
            .expect("declared export")
            .id()
    };
    let set_value = export("setValue");
    let read_value = export("readValue");
    let scratch = Scratch::new("populated-recovery");
    let instance = provision_from(scratch.dir(), &image);
    {
        let AttachOutcome::AlreadyActive(mut attachment) =
            attach(scratch.dir(), prepare(image.clone())).expect("attach")
        else {
            panic!("provisioned binding")
        };
        assert!(matches!(
            run_export(
                &mut attachment,
                set_value,
                vec![Value::Int(7), Value::Int(42)]
            ),
            Some(DurableRun::Ran(Ok(None)))
        ));
    }
    let before =
        marrow_lifecycle::audit(scratch.dir(), prepare(image.clone())).expect("before audit");
    assert_eq!(before.summary.entries, 1);
    let head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("head");
    let receipt =
        marrow_lifecycle::recover(scratch.dir(), prepare(image.clone())).expect("recovery");
    assert_eq!(receipt.instance, instance);
    assert_eq!(receipt.image_id, image.image_id());
    assert_eq!(
        std::fs::read(scratch.dir().join(HEAD_FILE)).expect("head unchanged"),
        head
    );
    let after =
        marrow_lifecycle::audit(scratch.dir(), prepare(image.clone())).expect("after audit");
    assert!(after.is_clean());
    assert_eq!(after.digest, before.digest);
    assert_eq!(after.summary.entries, 1);
    let AttachOutcome::AlreadyActive(mut attachment) =
        attach(scratch.dir(), prepare(image)).expect("attach recovered")
    else {
        panic!("recovery does not rebind")
    };
    assert!(matches!(
        run_export(&mut attachment, read_value, vec![Value::Int(7)]),
        Some(DurableRun::Ran(Ok(Some(Value::Int(42)))))
    ));
}

#[test]
fn accepted_numbering_preserves_values_after_field_insertion() {
    use marrow_vm::{DurableRun, Value, run_export};

    let source = format!(
        "{BASE_SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
    );
    let source = source.replace(
        "store ^counters[id: int]: Counter",
        "store ^counters[id: int]: Counter {\n    index byValue[value] unique\n}",
    );
    let old_ids = BASE_IDS.replace(
        "high-water 0",
        "id index counters.byValue 11111111111111111111111111111111\nhigh-water 0",
    );
    let old = compile(&source, &old_ids);
    for (anchor, replacement, fresh_extra_number) in [
        (
            "required value: int",
            "extra: int\n    required value: int",
            1,
        ),
        ("label: string", "label: string\n    extra: int", 3),
    ] {
        let inserted_source = source.replace(anchor, replacement)
            + r#"
pub fn readExtra(n: int): int {
    return ^counters[n].extra ?? -1
}
pub fn setExtra(n: int, v: int): bool {
    transaction {
        place counter = ^counters[n]
        if not exists(counter) {
            return false
        }
        counter.extra = v
    }
    return true
}
"#;
        let inserted_ids = old_ids.replace(
            "high-water 0",
            "id field Counter.extra 10101010101010101010101010101010\nhigh-water 0",
        );
        let inserted_bytes = compile_files(&[("src/main.mw", &inserted_source)], &inserted_ids);
        let inserted = verify(&inserted_bytes).expect("verify inserted program");
        let export = |image: &VerifiedImage, name: &str| {
            image
                .exports()
                .iter()
                .find(|export| {
                    image
                        .function(export.function())
                        .expect("verified function")
                        .body()
                        .name()
                        == name
                })
                .expect("declared export")
                .id()
        };
        // Retain the original store on a failed assertion for independent byte inspection.
        let scratch = std::mem::ManuallyDrop::new(Scratch::new("accepted-numbering"));
        eprintln!("accepted-numbering scratch: {}", scratch.dir().display());
        provision_from(scratch.dir(), &old);
        {
            let AttachOutcome::AlreadyActive(mut attachment) =
                attach(scratch.dir(), prepare(old.clone())).expect("old attach")
            else {
                panic!("provisioned binding")
            };
            assert!(matches!(
                run_export(
                    &mut attachment,
                    export(&old, "setValue"),
                    vec![Value::Int(7), Value::Int(42)]
                ),
                Some(DurableRun::Ran(Ok(None)))
            ));
        }
        let old_head = open_head(scratch.dir(), &old);
        let extra_id = marrow_image::LedgerIdBytes::from_bytes([0x10; 16]);
        let mut ids: Vec<_> = old_head
            .head_map
            .entries()
            .iter()
            .map(|entry| entry.ledger_id)
            .collect();
        ids.push(extra_id);
        let accepted = marrow_lifecycle::HeadMap::assign(&ids).expect("extended bijection");
        for entry in old_head.head_map.entries() {
            assert_eq!(accepted.number_of(&entry.ledger_id), Some(entry.number));
        }
        assert_eq!(
            accepted.number_of(&extra_id),
            Some(old_head.head_map.next_number())
        );
        assert_eq!(
            head_map(&inserted).expect("fresh map").number_of(&extra_id),
            Some(fresh_extra_number)
        );
        // Assemble the prospective accepted state without claiming update publication.
        // Only Head changes; existing value cells stay exactly where the old program put them.
        let head = LogicalHead::provision(
            active_binding(&inserted),
            marrow_lifecycle::accepted_ceiling(&inserted),
            accepted,
        );
        std::fs::write(scratch.dir().join(HEAD_FILE), head.encode()).expect("accepted head");
        let AttachOutcome::AlreadyActive(mut attachment) =
            attach(scratch.dir(), prepare(inserted.clone())).expect("accepted-map attach")
        else {
            panic!("accepted image is already active")
        };
        for (name, expected) in [("readValue", 42), ("readExtra", -1)] {
            assert!(matches!(
                run_export(&mut attachment, export(&inserted, name), vec![Value::Int(7)]),
                Some(DurableRun::Ran(Ok(Some(Value::Int(value))))) if value == expected
            ));
        }
        assert!(matches!(
            run_export(
                &mut attachment,
                export(&inserted, "setExtra"),
                vec![Value::Int(7), Value::Int(77)]
            ),
            Some(DurableRun::Ran(Ok(Some(Value::Bool(true)))))
        ));
        for (name, expected) in [("readValue", 42), ("readExtra", 77)] {
            assert!(matches!(
                run_export(&mut attachment, export(&inserted, name), vec![Value::Int(7)]),
                Some(DurableRun::Ran(Ok(Some(Value::Int(value))))) if value == expected
            ));
        }
        drop(attachment);
        let before_recovery = marrow_lifecycle::audit(scratch.dir(), prepare(inserted.clone()))
            .expect("accepted-map audit");
        assert!(before_recovery.is_clean());
        assert_eq!(before_recovery.summary.index_cells, 1);
        marrow_lifecycle::recover(scratch.dir(), prepare(inserted.clone()))
            .expect("accepted-map recovery");
        let recovered = marrow_lifecycle::audit(scratch.dir(), prepare(inserted.clone()))
            .expect("recovered audit");
        assert_eq!(recovered.digest, before_recovery.digest);
        let parent = scratch.dir().parent().expect("scratch parent");
        let backup_path = parent.join("backup.mwb");
        let backup = marrow_lifecycle::backup(scratch.dir(), &inserted_bytes, &backup_path)
            .expect("accepted-map backup");
        assert_eq!(backup.audit.digest, recovered.digest);
        let restored_path = parent.join("restored");
        let restored = marrow_lifecycle::restore(
            &mut std::fs::File::open(&backup_path).expect("backup input"),
            &restored_path,
        )
        .expect("accepted-map restore");
        assert_eq!(restored.audit.digest, recovered.digest);
        assert_ne!(restored.audit.instance, recovered.instance);
        assert_eq!(restored.audit.summary.index_cells, 1);
        for path in [scratch.dir(), restored_path.as_path()] {
            let AttachOutcome::AlreadyActive(mut attachment) =
                attach(path, prepare(inserted.clone())).expect("reconstructed attachment")
            else {
                panic!("reconstruction keeps accepted binding")
            };
            assert_eq!(attachment.head().head_map, head.head_map);
            for (name, expected) in [("readValue", 42), ("readExtra", 77)] {
                assert!(
                    matches!(run_export(&mut attachment, export(&inserted, name), vec![Value::Int(7)]), Some(DurableRun::Ran(Ok(Some(Value::Int(value))))) if value == expected)
                );
            }
        }
        drop(std::mem::ManuallyDrop::into_inner(scratch));
    }
}

#[cfg(unix)]
#[test]
fn rebind_preserves_an_occupied_replacement_slot() {
    use marrow_lifecycle::{AdmissionError, AdmissionFault, OpenError, StoreEntry};
    use std::os::unix::fs::MetadataExt;
    let image = compile(BASE_SOURCE, BASE_IDS);
    let edited = compile(&BASE_SOURCE.replace("?? 0", "?? 1"), BASE_IDS);
    for (slot, entry) in [
        ("envelope.replacing", StoreEntry::Envelope),
        ("head.replacing", StoreEntry::Head),
    ] {
        for shape in ["empty", "partial", "symlink", "directory", "hardlink"] {
            let scratch = Scratch::new("replacement-shape");
            provision_from(scratch.dir(), &image);
            let path = scratch.dir().join(slot);
            let peer = scratch.dir().join("unrelated");
            std::fs::write(&peer, b"unrelated bytes").expect("peer");
            match shape {
                "empty" => std::fs::write(&path, b"").expect("empty sibling"),
                "partial" => std::fs::write(&path, b"partial").expect("partial sibling"),
                "symlink" => std::os::unix::fs::symlink(&peer, &path).expect("symlink sibling"),
                "directory" => std::fs::create_dir(&path).expect("directory sibling"),
                "hardlink" => std::fs::hard_link(&peer, &path).expect("hardlink sibling"),
                _ => unreachable!(),
            }
            let before = std::fs::symlink_metadata(&path).expect("sibling metadata");
            let head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("head");
            let envelope = std::fs::read(scratch.dir().join(marrow_lifecycle::ENVELOPE_FILE))
                .expect("envelope");
            let error = attach(scratch.dir(), prepare(edited.clone()))
                .err()
                .expect("occupied slot refuses");
            assert!(
                matches!(error, LifecycleError::Metadata(AdmissionError {
                entry: found,
                fault: AdmissionFault::Custody(marrow_fs_journal::CustodyError::AlreadyExists { .. }),
            }) if found == entry),
                "{slot} {shape}: {error:?}"
            );
            let after = std::fs::symlink_metadata(&path).expect("retained sibling");
            assert_eq!(
                (
                    after.dev(),
                    after.ino(),
                    after.mode(),
                    after.nlink(),
                    after.len()
                ),
                (
                    before.dev(),
                    before.ino(),
                    before.mode(),
                    before.nlink(),
                    before.len()
                )
            );
            assert_eq!(
                std::fs::read(&peer).expect("peer unchanged"),
                b"unrelated bytes"
            );
            if shape == "partial" {
                assert_eq!(std::fs::read(&path).expect("partial unchanged"), b"partial");
            }
            if shape == "symlink" {
                assert_eq!(std::fs::read_link(&path).expect("link unchanged"), peer);
            }
            assert_eq!(
                std::fs::read(scratch.dir().join(HEAD_FILE)).expect("head unchanged"),
                head
            );
            if entry == StoreEntry::Envelope {
                assert_eq!(
                    std::fs::read(scratch.dir().join(marrow_lifecycle::ENVELOPE_FILE))
                        .expect("envelope unchanged"),
                    envelope
                );
            } else {
                assert!(matches!(
                    attach(scratch.dir(), prepare(image.clone())),
                    Err(LifecycleError::Open(OpenError::ActivationRequired { .. }))
                ));
            }
        }
    }
}

/// Provision a fresh store at `dir` bound to `image`.
#[test]
fn old_image_binding_refuses_active_and_rebind_before_engine_open() {
    let image = compile(BASE_SOURCE, BASE_IDS);
    let edited = compile(&BASE_SOURCE.replace("?? 0", "?? 1"), BASE_IDS);
    for presented in [&image, &edited] {
        for broken_engine in [false, true] {
            let scratch = Scratch::new("old-image-binding");
            let dir = scratch.dir();
            provision_from(dir, &image);
            let head_path = dir.join(marrow_lifecycle::HEAD_FILE);
            let mut head = LogicalHead::decode(&std::fs::read(&head_path).expect("head"))
                .expect("current head");
            head.binding.image_format_version = 0;
            std::fs::write(&head_path, head.encode()).expect("old image binding");
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
            let error = attach(dir, prepare(presented.clone()))
                .err()
                .expect("an old image binding must not be attached or rebound");
            assert_eq!(error.code(), Code::StoreFormatVersion);
            assert!(matches!(
                error,
                LifecycleError::Open(marrow_lifecycle::OpenError::Admission(
                    marrow_lifecycle::AdmissionError {
                        entry: marrow_lifecycle::StoreEntry::Head,
                        fault: marrow_lifecycle::AdmissionFault::Format(
                            marrow_lifecycle::FormatError::UnsupportedImageVersion { found: 0 }
                        ),
                    }
                ))
            ));
            for (name, bytes) in before {
                assert_eq!(std::fs::read(dir.join(name)).expect("after refusal"), bytes);
            }
        }
    }
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
            verify(&compile_files(
                &[("src/main.mw", BASE_SOURCE), ("src/extra.mw", &relocated)],
                BASE_IDS,
            ))
            .expect("verify"),
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

/// The head-map numbering agrees node-for-node with the kernel's `number_store`: both walk
/// the durable graph in the same canonical split pre-order, so position `i` in both walks
/// must be the *same node* — same kind **and same ledger identity**, resolved here through
/// [`GRAPH_IDS`]'s explicit anchor table. This is the cross-crate enforcement artifact
/// against pre-order drift between the two independent numbering owners: a
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
    use marrow_vm::{DurableRun, Value, run_export};
    let scratch = Scratch::new("rebind");
    let source = format!(
        "{BASE_SOURCE}\npub fn setValue(n: int, v: int) {{ transaction {{ ^counters[n] = Counter(value: v) }} }}\n"
    );
    let image = compile(&source, BASE_IDS);
    let export = |image: &VerifiedImage, name: &str| {
        image
            .exports()
            .iter()
            .find(|export| {
                image
                    .function(export.function())
                    .expect("function")
                    .body()
                    .name()
                    == name
            })
            .expect("export")
            .id()
    };
    let instance = provision_from(scratch.dir(), &image);
    let original = active_binding(&image);
    {
        let AttachOutcome::AlreadyActive(mut attachment) =
            attach(scratch.dir(), prepare(image.clone())).expect("attach")
        else {
            panic!("provisioned binding")
        };
        assert!(matches!(
            run_export(
                &mut attachment,
                export(&image, "setValue"),
                vec![Value::Int(7), Value::Int(42)]
            ),
            Some(DurableRun::Ran(Ok(None)))
        ));
    }

    // A body-only edit: the fallback default changes, so the image bytes differ, but the
    // export signature, the durable contract, and the ceiling are all preserved.
    let edited_source = source.replace("?? 0", "?? 1");
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
            mut attachment,
            receipt,
        } => {
            let read = export(&edited, "readValue");
            assert!(matches!(
                run_export(&mut attachment, read, vec![Value::Int(7)]),
                Some(DurableRun::Ran(Ok(Some(Value::Int(42)))))
            ));
            assert!(matches!(
                run_export(&mut attachment, read, vec![Value::Int(8)]),
                Some(DurableRun::Ran(Ok(Some(Value::Int(1)))))
            ));
            assert!(matches!(
                run_export(
                    &mut attachment,
                    export(&edited, "setValue"),
                    vec![Value::Int(8), Value::Int(77)]
                ),
                Some(DurableRun::Ran(Ok(None)))
            ));
            assert!(matches!(
                run_export(&mut attachment, read, vec![Value::Int(8)]),
                Some(DurableRun::Ran(Ok(Some(Value::Int(77)))))
            ));
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

/// Replace one accepted identity with a foreign ID while preserving a valid bijection
/// and image binding. Return the image identity that the Head no longer covers.
fn foreign_persisted_pin(dir: &Path, image: &VerifiedImage) -> marrow_image::LedgerIdBytes {
    let map = head_map(image).expect("head map");
    let mut ids: Vec<marrow_image::LedgerIdBytes> =
        map.entries().iter().map(|entry| entry.ledger_id).collect();
    let missing = ids[0];
    ids[0] = marrow_image::LedgerIdBytes::from_bytes([0xee; 16]);
    let foreign = marrow_lifecycle::HeadMap::assign(&ids).expect("a foreign bijection assigns");
    let forged = LogicalHead::provision(
        active_binding(image),
        marrow_lifecycle::accepted_ceiling(image),
        foreign,
    );
    std::fs::write(dir.join(HEAD_FILE), forged.encode()).expect("write foreign head");
    missing
}

/// The pin family covers every serving outcome: an attach serves a store only as
/// already-active or as a binding-only rebind, and both arms are fenced by a foreign-pin
/// fixture below. A new [`AttachOutcome`] variant fails this match until the family covers
/// it too.
fn _pin_family_covers_every_serving_outcome(outcome: AttachOutcome) {
    match outcome {
        AttachOutcome::AlreadyActive(_) => (),
        AttachOutcome::Rebound { .. } => (),
    }
}

/// A framed, resealed Head with a missing image identity must refuse before service.
#[test]
fn a_store_with_a_foreign_head_map_pin_is_refused_at_attach() {
    let scratch = Scratch::new("pin-foreign");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    let first_id = foreign_persisted_pin(scratch.dir(), &image);

    match attach(scratch.dir(), prepare(image)) {
        Err(LifecycleError::HeadMapPin(refusal)) => {
            assert_eq!(
                refusal.code(),
                Code::StoreCorruption,
                "fail-closed, recovery-shaped"
            );
            // Refusal names the first uncovered image identity in structural order.
            assert_eq!(
                refusal.disagreement,
                PinDisagreement::Missing {
                    ledger_id: first_id,
                },
            );
        }
        Err(other) => panic!(
            "expected the pin refusal, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!(
            "a store whose persisted head-map pin disagrees with the derived numbering must \
             be refused, but attach served it"
        ),
    }
}

/// The pin refusal precedes any engine call: with the engine file replaced by garbage — an
/// engine open would fail loudly — a foreign pin still surfaces as the pin refusal, so the
/// disagreement is decided strictly before the engine (and therefore before any read or
/// mutation) is reached.
#[test]
fn the_pin_refusal_precedes_any_engine_call() {
    let scratch = Scratch::new("pin-before-engine");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    foreign_persisted_pin(scratch.dir(), &image);
    std::fs::write(
        scratch.dir().join(marrow_lifecycle::ENGINE_FILE),
        b"not an engine",
    )
    .expect("corrupt the engine file");

    match attach(scratch.dir(), prepare(image)) {
        Err(LifecycleError::HeadMapPin(_)) => {}
        Err(other) => panic!(
            "the pin must refuse before the engine is touched, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("a foreign pin over a garbage engine was served"),
    }
}

/// The rebind arm is fenced too: a body-only edit (the binding-only rebind case) against a
/// foreign pin is refused without rewriting the head or envelope, and the refusal releases
/// the single-owner lock — restoring the true head lets the same rebind commit.
#[test]
fn a_rebind_over_a_foreign_pin_is_refused_without_a_write() {
    let scratch = Scratch::new("pin-rebind");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    let true_head = std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read true head");
    foreign_persisted_pin(scratch.dir(), &image);

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
        Err(other) => panic!(
            "expected the pin refusal, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("a rebind over a foreign pin was served"),
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
/// the pin comparison does not preempt the typed contract-changed refusal: over a foreign
/// pin, an image with an evolved contract is still refused as `store.contract_changed` — and
/// the store is not served on that path either.
#[test]
fn a_contract_change_over_a_foreign_pin_stays_a_contract_refusal() {
    let scratch = Scratch::new("pin-contract");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);
    foreign_persisted_pin(scratch.dir(), &image);

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
        Err(other) => panic!(
            "expected the contract refusal, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("a contract change must be refused"),
    }
}

/// Accepted address high-water describes lifetime allocation, independently of the
/// current node count. A valid gap does not change existing addresses.
#[test]
fn accepted_high_water_is_independent_of_current_node_count() {
    let scratch = Scratch::new("pin-high-water");
    let image = compile(GRAPH_SOURCE, GRAPH_IDS);
    provision_from(scratch.dir(), &image);

    // The head map's high-water u32 sits right after the fixed head prefix:
    // magic(4)+ver(1)+imgfmt(1)+3×id(32)+commit(8)+ddig(32)+ddpos(8) = 150.
    let head_path = scratch.dir().join(HEAD_FILE);
    let mut bytes = std::fs::read(&head_path).expect("read head");
    let map_start = 4 + 1 + 1 + 32 * 3 + 8 + 32 + 8;
    bytes[map_start..map_start + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    let body_len = bytes.len() - 32;
    let resealed = marrow_image::StoreHeadDigest::compute(&bytes[..body_len]);
    bytes[body_len..].copy_from_slice(resealed.bytes());
    std::fs::write(&head_path, &bytes).expect("write forged head");

    match attach(scratch.dir(), prepare(image)) {
        Ok(AttachOutcome::AlreadyActive(attachment)) => {
            assert_eq!(attachment.head().head_map.next_number(), u32::MAX);
        }
        Err(other) => panic!(
            "accepted high-water refused, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("the exact accepted image must not rebind"),
    }
}

/// An incompatible image has no admitted physical mapping for this store. Refuse it
/// before engine opening rather than constructing a handle that must not be served.
#[test]
fn a_changed_contract_is_refused_before_engine_open() {
    let scratch = std::mem::ManuallyDrop::new(Scratch::new("contract-garbage-engine"));
    eprintln!("contract-refusal scratch: {}", scratch.dir().display());
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
        Err(LifecycleError::ContractChanged(refusal)) => {
            assert_eq!(refusal.changed, ChangedFact::DurableContract);
        }
        Err(other) => panic!(
            "the incompatible contract must refuse before engine opening, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("an incompatible contract must not open"),
    }
    drop(std::mem::ManuallyDrop::into_inner(scratch));
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
            assert_eq!(refusal.code(), Code::StoreContractChanged);
            assert_ne!(refusal.code(), Code::StoreCorruption);
        }
        Err(other) => panic!(
            "expected an interface refusal, got code {}",
            other.code().as_str()
        ),
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
            assert_eq!(refusal.code(), Code::StoreContractChanged);
        }
        Err(other) => panic!(
            "expected a durable-contract refusal, got code {}",
            other.code().as_str()
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
                assert_eq!(refusal.code(), Code::StoreContractChanged, "{fact}");
            }
            Err(other) => panic!(
                "{fact}: expected a durable-contract refusal, got code {}",
                other.code().as_str()
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
            assert_eq!(refusal.code(), Code::StoreContractChanged);
        }
        Err(other) => panic!(
            "dropping a key column must be a durable-contract refusal, got code {}",
            other.code().as_str()
        ),
        Ok(_) => panic!("the key tuple narrowed but the store was served"),
    }
    assert_eq!(
        std::fs::read(scratch.dir().join(HEAD_FILE)).expect("read head"),
        head,
        "the refusal rewrote the head it refused",
    );
}

/// Old layouts refuse before exact-active attachment or a body-only rebind can
/// open the engine. A broken engine makes the ordering observable.
#[test]
fn generation_one_refuses_active_and_rebind_before_engine_open() {
    let image = compile(BASE_SOURCE, BASE_IDS);
    let edited = compile(&BASE_SOURCE.replace("?? 0", "?? 1"), BASE_IDS);
    assert_ne!(image.image_id(), edited.image_id());
    assert!(active_binding(&image).facts_equal(&active_binding(&edited)));
    for presented in [&image, &edited] {
        for broken_engine in [true, false] {
            let scratch = Scratch::new("old-generation");
            let dir = scratch.dir();
            provision_from(dir, &image);
            let head_path = dir.join(marrow_lifecycle::HEAD_FILE);
            let current_head = std::fs::read(&head_path).expect("current head");
            let mut old_head = current_head.clone();
            old_head[4] = 1;
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
            let outcome = attach(dir, prepare(presented.clone()));
            let error = match outcome {
                Err(error) => error,
                Ok(_) => panic!("an older layout must not be attached or rebound"),
            };
            assert_eq!(error.code(), Code::StoreFormatVersion);
            assert!(matches!(
                error,
                LifecycleError::Open(marrow_lifecycle::OpenError::Admission(
                    marrow_lifecycle::AdmissionError {
                        entry: marrow_lifecycle::StoreEntry::Head,
                        fault: marrow_lifecycle::AdmissionFault::Format(
                            marrow_lifecycle::FormatError::UnknownVersion { found: 1 }
                        ),
                    }
                ))
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
                let outcome = attach(dir, prepare(presented.clone())).expect("lock released");
                assert_eq!(
                    matches!(outcome, AttachOutcome::Rebound { .. }),
                    presented.image_id() != image.image_id()
                );
            }
        }
    }
}
