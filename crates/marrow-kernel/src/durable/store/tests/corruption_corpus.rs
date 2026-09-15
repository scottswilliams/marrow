//! Store-level byte injection: the corrupt, valid and orphan corpus.

use super::*;

//
// These seed cells directly through the engine seam — not through the session
// ops — to place the store in states the ops alone cannot construct, then read
// through a coherent session. They pin the corrupt/valid boundary the bounded
// prefix probe draws once a branch subtree can nest below a node: a marker-absent
// node with a legitimate keyed descendant is *valid* (descendant-only,
// payload-absent), while a marker-absent node with one of its *own* field leaves
// is *corrupt* — and the own-leaf corruption is surfaced ahead of the legitimate
// descendant (the `0x10 < 0x30` precedence).

/// VALID: a branch child (marker plus its own `text` leaf) under an absent root is a
/// legitimate descendant-only node. A whole read of the root is payload-absent, not
/// corruption, and the branch entry reads back — the byte-injected counterpart of the
/// ops-built descendant-only case.
#[test]
fn an_injected_descendant_only_node_reads_payload_absent_not_corruption() {
    let branch_stem = physical::marker_key(branch_num(&branch_schema().0, &[0]), &[ks("a"), ki(7)]);
    let mut store = injected_branch_store(&[
        (branch_stem.clone(), physical::MARKER_VALUE.to_vec()),
        (
            physical::stem_field_leaf(&branch_stem, branch_field_num(&branch_schema().0, &[0], 0)),
            b"hi".to_vec(),
        ),
    ]);
    let book = KeyScalar::Str("a".into());
    let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    assert_eq!(
        read.read_entry(&root, std::slice::from_ref(&book)),
        Ok(None),
        "a marker-absent node with only a keyed descendant is a valid descendant-only node",
    );
    assert_eq!(
        read.presence(&root, std::slice::from_ref(&book)),
        Ok(Presence::Absent),
    );
    let branch = read.site(1);
    assert_eq!(
        read.read_entry(&branch, &note),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))],
        })),
    );
}

/// A root's own field without its marker is corrupt even when a child exists.
/// The bounded own-prefix probe encounters that orphan; the child occupies a
/// separate family and cannot make the root payload valid.
#[test]
fn an_injected_root_own_leaf_without_a_marker_is_corruption_even_with_a_descendant() {
    let stem = book_stem("a");
    let branch_stem = physical::marker_key(branch_num(&branch_schema().0, &[0]), &[ks("a"), ki(7)]);
    let mut store = injected_branch_store(&[
        // The root's own `title` leaf, with no root marker: an orphan.
        (
            physical::stem_field_leaf(&stem, field_num(&branch_schema().0, 0)),
            b"Book A".to_vec(),
        ),
        // A legitimate branch descendant below the same (markerless) root.
        (branch_stem.clone(), physical::MARKER_VALUE.to_vec()),
        (
            physical::stem_field_leaf(&branch_stem, branch_field_num(&branch_schema().0, &[0], 0)),
            b"hi".to_vec(),
        ),
    ]);
    let book = KeyScalar::Str("a".into());
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    assert_eq!(
        read.read_entry(&root, std::slice::from_ref(&book)),
        Err(KernelFault::Corruption),
        "an orphan own leaf is surfaced ahead of a legitimate descendant",
    );
}

/// ORPHAN (branch level): a branch child that has its own `text` leaf but no branch
/// marker is corrupt, exactly as a root orphan is — the marker/field law holds one
/// level down.
#[test]
fn an_injected_branch_own_leaf_without_a_branch_marker_is_corruption() {
    let stem = book_stem("a");
    let branch_stem = physical::marker_key(branch_num(&branch_schema().0, &[0]), &[ks("a"), ki(7)]);
    let mut store = injected_branch_store(&[
        // The root has a real payload, so the root itself is well-formed.
        (stem.clone(), physical::MARKER_VALUE.to_vec()),
        (
            physical::stem_field_leaf(&stem, field_num(&branch_schema().0, 0)),
            b"Book A".to_vec(),
        ),
        // The branch child's own leaf with no branch marker: an orphan.
        (
            physical::stem_field_leaf(&branch_stem, branch_field_num(&branch_schema().0, &[0], 0)),
            b"hi".to_vec(),
        ),
    ]);
    let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let branch = read.site(1);
    assert_eq!(
        read.read_entry(&branch, &note),
        Err(KernelFault::Corruption),
        "a branch own leaf without its branch marker is corruption",
    );
}

/// The descendant-skip law of the bounded acquisition over a run of descendant-only
/// entries: it freezes only payload-bearing (marker-present) entries, seeking a
/// descendant-only entry's whole subtree in one cursor step. Present entries `k1`
/// and `k4` bracket two descendant-only entries `k2` and `k3` — each a markerless
/// root carrying only a keyed branch child — injected directly so the ops cannot
/// construct the state. The acquisition from the start freezes `[k1, k4]`, skipping
/// both; and an inclusive `from` inside the descendant-only run still resolves to
/// `k4`, so the skip does not depend on starting at a present entry.
#[test]
fn a_bounded_acquisition_skips_a_run_of_descendant_only_entries_between_siblings() {
    let mut cells = Vec::new();
    // Present entries: a root marker plus its `title` leaf.
    for present in ["k1", "k4"] {
        let stem = book_stem(present);
        cells.push((stem.clone(), physical::MARKER_VALUE.to_vec()));
        cells.push((
            physical::stem_field_leaf(&stem, field_num(&branch_schema().0, 0)),
            b"T".to_vec(),
        ));
    }
    // Descendant-only entries: a branch child (marker plus `text` leaf) with no
    // root marker, so the root has children but no visitable payload.
    for descendant_only in ["k2", "k3"] {
        let branch_stem = physical::marker_key(
            branch_num(&branch_schema().0, &[0]),
            &[ks(descendant_only), ki(7)],
        );
        cells.push((branch_stem.clone(), physical::MARKER_VALUE.to_vec()));
        cells.push((
            physical::stem_field_leaf(&branch_stem, branch_field_num(&branch_schema().0, &[0], 0)),
            b"hi".to_vec(),
        ));
    }
    let mut store = injected_branch_store(&cells);
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    let k = |s: &str| KeyScalar::Str(s.into());

    // From the start: the two present siblings, the two descendant-only entries
    // skipped in one seek run.
    assert_eq!(
        read.iterate_bounded(&root, &[], None, bound(8))
            .expect("iterate"),
        BoundedKeys {
            keys: vec![k("k1"), k("k4")],
            more: false,
        },
    );

    // An inclusive `from` inside the descendant-only run — at `k2`, the first of
    // the two, or `k3`, the second — resolves to `k4` just as a `from` at the
    // present sibling before them does.
    for start in ["k2", "k3"] {
        assert_eq!(
            read.iterate_bounded(&root, &[], Some(k(start)), bound(8))
                .expect("iterate")
                .keys,
            vec![k("k4")],
            "an inclusive from inside the descendant-only run still yields the next present key",
        );
    }
}
