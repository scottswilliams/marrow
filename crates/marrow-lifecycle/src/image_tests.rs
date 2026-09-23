//! Private image/projection correspondence controls.

use super::*;
use crate as marrow_lifecycle;
use marrow_test_programs::program::compile_files;
use marrow_test_support::graph_corpus::*;

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

/// A different projection order still resolves each durable identity to its accepted
/// address; fresh preorder numbers would incorrectly move the second root to zero.
#[test]
fn accepted_numbers_follow_identity_across_projection_order() {
    let bytes = compile_files(&[("src/main.mw", GRAPH_SOURCE)], GRAPH_IDS);
    let image = marrow_verify::verify(&bytes).expect("verify");

    let derived = derive_projection(&image).expect("projection");
    let mut builder = marrow_kernel::durable::StoreProjection::builder();
    for schema in derived.roots().iter().rev() {
        builder.root(schema.clone());
    }
    let drifted = builder
        .finish()
        .expect("the swapped-root projection builds");

    // The second root keeps its physical number after its projection position moves.
    let map = head_map(&image).expect("head map");
    let tags_root = marrow_image::LedgerIdBytes::from_bytes([0x4b; 16]);
    let pinned_tags = map
        .number_of(&tags_root)
        .expect("the pin binds the tags root");
    let numbers = derive_projection_nodes(&image, &drifted)
        .expect("correspondence")
        .accepted_numbers(&map)
        .expect("accepted mapping");
    assert_eq!(numbers[0], pinned_tags);
    assert_ne!(numbers[0], 0);
    assert_eq!(numbers.len(), map.len());
}

/// A forged projection changing a group into a branch fails the private correspondence
/// check even when its paths and physical numbers match the image.
#[test]
fn a_group_respelled_as_a_branch_projection_is_refused_by_kind() {
    use marrow_kernel::codec::value::ScalarKind;

    let bytes = compile_files(&[("src/main.mw", GROUPSWAP_SOURCE)], GROUPSWAP_IDS);
    let image = marrow_verify::verify(&bytes).expect("verify");

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
    match check_correspondence(&image, &swapped, &map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Kind {
                path: "^books.details".to_string(),
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

    let bytes = compile_files(&[("src/main.mw", GROUPSWAP_SOURCE)], GROUPSWAP_IDS);
    let image = marrow_verify::verify(&bytes).expect("verify");
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
    match check_correspondence(&image, &truncated, &truncated_map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Uncovered {
                ledger_id: marrow_image::LedgerIdBytes::from_bytes([0x20; 16]),
                path: Some("^books.details".to_string()),
            },
        ),
        Ok(()) => panic!("an under-covering projection must refuse"),
    }
}

/// Shared declaration identities cannot cover missing occurrences of a field, group or
/// branch under another root. The nested-branch case separately checks traversal depth.
/// A truncated map cannot hide these omissions because correspondence is checked first.
#[test]
fn coverage_is_decided_over_occurrence_identity_not_declaration_identity() {
    let bytes = compile_files(
        &[("src/main.mw", SHARED_PRODUCT_SOURCE)],
        SHARED_PRODUCT_IDS,
    );
    let image = marrow_verify::verify(&bytes).expect("verify");
    let map = marrow_lifecycle::HeadMap::assign(&[
        marrow_image::LedgerIdBytes::from_bytes([0x52; 16]),
        marrow_image::LedgerIdBytes::from_bytes([0x54; 16]),
    ])
    .expect("the root-only map assigns");

    // Each case gives `^a` the parts left of the bar and `^b` those right of it.
    for (kind, split, ledger, path) in [
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
        match check_correspondence(&image, &partial, &map) {
            Err(refusal) => assert_eq!(
                refusal.disagreement,
                PinDisagreement::Uncovered {
                    ledger_id: marrow_image::LedgerIdBytes::from_bytes([ledger; 16]),
                    path: Some(path.to_string()),
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

    let bytes = compile_files(&[("src/main.mw", GRAPH_SOURCE)], GRAPH_IDS);
    let image = marrow_verify::verify(&bytes).expect("verify");
    let schema = marrow_kernel::durable::StoreSchemaBuilder::root("phantom", vec![ScalarKind::Int])
        .finish()
        .expect("a rootonly schema builds");
    let mut builder = marrow_kernel::durable::StoreProjection::builder();
    builder.root(schema);
    let foreign = builder.finish().expect("the projection builds");

    let map = head_map(&image).expect("head map");
    match check_correspondence(&image, &foreign, &map) {
        Err(refusal) => assert_eq!(
            refusal.disagreement,
            PinDisagreement::Unnamed {
                path: "^phantom".to_string()
            },
        ),
        Ok(()) => panic!("an unnamed store node must refuse"),
    }
}

/// A valid resealed mapping defines its addresses. The correspondence check verifies
/// coverage, not historical authorship; even a same-type permutation retains its numbers.
#[test]
fn accepted_numbers_preserve_the_declared_map() {
    let bytes = compile_files(&[("src/main.mw", GRAPH_SOURCE)], GRAPH_IDS);
    let image = marrow_verify::verify(&bytes).expect("verify");
    let projection = derive_projection(&image).expect("projection");
    let map = head_map(&image).expect("head map");
    assert_eq!(check_correspondence(&image, &projection, &map), Ok(()));

    let mut ids: Vec<marrow_image::LedgerIdBytes> =
        map.entries().iter().map(|entry| entry.ledger_id).collect();
    ids.swap(1, 2);
    let permuted = marrow_lifecycle::HeadMap::assign(&ids).expect("assign");
    let numbers = derive_projection_nodes(&image, &projection)
        .expect("correspondence")
        .accepted_numbers(&permuted)
        .expect("accepted permutation");
    assert_eq!(numbers, [0, 2, 1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
}

#[test]
fn accepted_mapping_refuses_an_unreached_binding() {
    let bytes = compile_files(&[("src/main.mw", GRAPH_SOURCE)], GRAPH_IDS);
    let image = marrow_verify::verify(&bytes).expect("verify");
    let projection = derive_projection(&image).expect("projection");
    let map = head_map(&image).expect("head map");
    let foreign = marrow_image::LedgerIdBytes::from_bytes([0xff; 16]);
    let mut ids: Vec<_> = map.entries().iter().map(|entry| entry.ledger_id).collect();
    ids.push(foreign);
    let extended = HeadMap::assign(&ids).expect("valid map with an extra binding");
    let error = check_correspondence(&image, &projection, &extended)
        .expect_err("every accepted binding must be reached");
    assert_eq!(
        error.disagreement,
        PinDisagreement::Unexpected {
            ledger_id: foreign,
            number: map.next_number(),
        }
    );
}

fn check_correspondence(
    image: &VerifiedImage,
    projection: &StoreProjection,
    persisted: &HeadMap,
) -> Result<(), HeadMapPinMismatch> {
    derive_projection_nodes(image, projection)?
        .accepted_numbers(persisted)
        .map(|_| ())
}
