//! Nested (multi-level) branches: multi-hop stems, the four-state probe at depth,
//! payload-only replace/erase at a sub-branch, and bounded traversal over an inner
//! layer. These pin level independence — a sub-branch node's marker/field/cursor
//! topology, slot classification, and replace/erase confinement behave at depth
//! exactly as at the root.

use super::*;

/// A four-level nested-branch schema: root `books`(Str) → branch `notes`(Int) →
/// sub-branch `tags`(Str) → sub-sub-branch `links`(Int). Each branch level carries a
/// required and a sparse field so the payload-only law (replace erases omitted own
/// fields; erase removes own cells; both preserve keyed descendants) is exercised
/// at depth. Sites: 0 root, 1 notes, 2 tags, 3
/// links, 4 tags.weight (sparse), 5 tags.label (required), 6 notes.color (sparse).
fn nested_schema() -> (StoreSchema, Vec<SiteTarget>) {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_branch("notes", vec![ScalarKind::Int]);
    builder.scalar_field("text", ScalarKind::Str, true);
    builder.scalar_field("color", ScalarKind::Str, false);
    builder.open_branch("tags", vec![ScalarKind::Str]);
    builder.scalar_field("label", ScalarKind::Str, true);
    builder.scalar_field("weight", ScalarKind::Int, false);
    builder.open_branch("links", vec![ScalarKind::Int]);
    builder.scalar_field("url", ScalarKind::Str, false);
    builder.close_branch();
    builder.close_branch();
    builder.close_branch();
    let schema = builder.finish().expect("the nested schema builds");
    let sites = vec![
        SiteTarget::whole_payload(),
        branch_entry(&[0]),
        branch_entry(&[0, 0]),
        branch_entry(&[0, 0, 0]),
        branch_field(&[0, 0], 1),
        branch_field(&[0, 0], 0),
        branch_field(&[0], 1),
    ];
    (schema, sites)
}

/// The physical marker stem of book `book` (level 0). `books` is the sole root: number 0.
fn nested_book_stem(book: &str) -> Vec<u8> {
    physical::marker_key(0, &[ks(book)])
}
/// The physical marker stem of note `note` under `book` (level 1).
fn nested_note_stem(book: &str, note: i64) -> Vec<u8> {
    physical::marker_key(branch_num(&nested_schema().0, &[0]), &[ks(book), ki(note)])
}
/// The physical marker stem of tag `tag` under `book`/`note` (level 2).
fn nested_tag_stem(book: &str, note: i64, tag: &str) -> Vec<u8> {
    physical::marker_key(
        branch_num(&nested_schema().0, &[0, 0]),
        &[ks(book), ki(note), ks(tag)],
    )
}

fn nested_store() -> DurableStore<MemoryEngine> {
    let (schema, sites) = nested_schema();
    DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites))
}

/// Seed raw `cells` into a fresh engine wrapped in the nested schema, so a read
/// session observes exactly the injected bytes — the multi-hop counterpart of
/// `injected_branch_store`, for states the ops cannot construct.
fn injected_nested_store(cells: &[(Vec<u8>, Vec<u8>)]) -> DurableStore<MemoryEngine> {
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        for (key, value) in cells {
            txn.put(key, value.clone()).expect("seed cell");
        }
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let (schema, sites) = nested_schema();
    DurableStore::from_engine(engine, project(&schema, sites))
}

/// Every raw cell whose key lies under `prefix`, as an owned map. A node's marker is a
/// byte-prefix of its whole subtree, so passing a node's marker stem captures exactly
/// that node and every descendant.
fn cells_under(
    store: &DurableStore<MemoryEngine>,
    prefix: &[u8],
) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
    all_cells(store)
        .into_iter()
        .filter(|(key, _)| key.starts_with(prefix))
        .collect()
}

/// Create a whole entry at `site` with `fields`, committing the transaction.
fn create_at(
    store: &mut DurableStore<MemoryEngine>,
    site: u16,
    keys: &[KeyScalar],
    fields: Vec<Option<ValueDomain>>,
) {
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .expect("txn session");
    let target = txn.site(site);
    assert_eq!(
        txn.create_entry(
            &target,
            keys,
            EntryValue {
                groups: Vec::new(),
                fields
            }
        )
        .expect("create"),
        CreateOutcome::Created,
    );
    assert!(matches!(txn.commit(), CommitResult::Committed));
}

/// A whole-entry create at a level-2 (tags) node writes its marker and own field
/// leaves under the multi-hop stem `books/book → notes/note → tags/tag`, and nowhere
/// shallower; the entry reads back through its multi-hop site. Marker, field leaf, and
/// the read path all resolve the same two-branch-hop stem.
#[test]
fn a_nested_branch_entry_addresses_its_multi_hop_stem_and_reads_back() {
    let mut store = nested_store();
    let tag = [ks("a"), ki(7), ks("x")];
    create_at(&mut store, 2, &tag, vec![vs("home"), None]);

    let cells = all_cells(&store);
    let tag_stem = nested_tag_stem("a", 7, "x");
    assert!(
        cells.contains_key(&tag_stem),
        "the tags marker sits at the multi-hop stem",
    );
    assert!(
        cells.contains_key(&physical::stem_field_leaf(
            &tag_stem,
            branch_field_num(&nested_schema().0, &[0, 0], 0)
        )),
        "the tags label leaf hangs off the multi-hop stem",
    );
    // A nested entry create writes no shallower marker: neither the parent note nor
    // the root book gains a payload marker.
    assert!(
        !cells.contains_key(&nested_note_stem("a", 7)),
        "the parent note node has no marker",
    );
    assert!(
        !cells.contains_key(&nested_book_stem("a")),
        "the root book node has no marker",
    );

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let tags = read.site(2);
    assert_eq!(
        read.read_entry(&tags, &tag),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![vs("home"), None],
        })),
        "the level-2 entry reads back through its multi-hop site",
    );
}

/// The four-state slot probe at a level-2 (tags) inner node: present (marker), absent
/// (nothing), descendant-only (a level-3 links child, no tags marker), each read
/// through the ops. The probe classifies the 3-hop stem exactly as it does the root.
#[test]
fn probe_slot_present_absent_and_descendant_only_at_a_level_two_node() {
    let mut store = nested_store();
    let present = [ks("a"), ki(1), ks("p")];
    let descendant_only = [ks("a"), ki(1), ks("d")];
    let absent = [ks("a"), ki(1), ks("z")];
    // Present: a tags entry with its required label.
    create_at(&mut store, 2, &present, vec![vs("home"), None]);
    // Descendant-only: a level-3 links child under tag "d", with no tags "d" marker.
    create_at(
        &mut store,
        3,
        &[ks("a"), ki(1), ks("d"), ki(100)],
        vec![vs("u")],
    );

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let tags = read.site(2);
    assert!(
        matches!(read.read_entry(&tags, &present), Ok(Some(_))),
        "a present inner node reads a payload",
    );
    assert_eq!(
        read.read_entry(&tags, &absent),
        Ok(None),
        "an absent inner node reads payload-absent",
    );
    assert_eq!(read.presence(&tags, &absent), Ok(Presence::Absent));
    assert_eq!(
        read.read_entry(&tags, &descendant_only),
        Ok(None),
        "a markerless inner node with only a keyed descendant reads payload-absent",
    );
    assert_eq!(read.presence(&tags, &descendant_only), Ok(Presence::Absent));
}

/// The fourth probe state at a level-2 node: an injected tags own field leaf (`label`)
/// with no tags marker is an orphan — corruption on a committed read, exactly as a
/// root orphan is. The marker/field law holds two levels down.
#[test]
fn an_injected_level_two_orphan_leaf_is_corruption() {
    let tag_stem = nested_tag_stem("a", 1, "x");
    let mut store = injected_nested_store(&[(
        physical::stem_field_leaf(&tag_stem, branch_field_num(&nested_schema().0, &[0, 0], 0)),
        b"home".to_vec(),
    )]);
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let tags = read.site(2);
    assert_eq!(
        read.read_entry(&tags, &[ks("a"), ki(1), ks("x")]),
        Err(KernelFault::Corruption),
        "a tags own leaf without its tags marker is corruption two levels down",
    );
}

/// The sub-branch uniform payload-only law for REPLACE: a whole replace of a level-1
/// branch entry erases its omitted own field and preserves its sub-branch subtree
/// byte-identically. The note's `color` is dropped and `text` replaced, while its
/// whole `tags` subtree (a tags entry with its own fields) survives untouched.
#[test]
fn a_replace_of_a_branch_entry_erases_omitted_fields_and_preserves_its_sub_branch_subtree() {
    let mut store = nested_store();
    let note = [ks("a"), ki(1)];
    let tag = [ks("a"), ki(1), ks("x")];
    create_at(&mut store, 1, &note, vec![vs("hi"), vs("red")]);
    create_at(&mut store, 2, &tag, vec![vs("home"), vi(5)]);
    let subtree_before = cells_under(&store, &nested_tag_stem("a", 1, "x"));

    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let notes = txn.site(1);
        txn.replace_entry(
            &notes,
            &note,
            EntryValue {
                groups: Vec::new(),
                fields: vec![vs("bye"), None],
            },
        )
        .expect("replace");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let after = all_cells(&store);
    assert!(
        !after.contains_key(&physical::stem_field_leaf(
            &nested_note_stem("a", 1),
            branch_field_num(&nested_schema().0, &[0], 1)
        )),
        "the replace erased the omitted color field",
    );
    assert_eq!(
        cells_under(&store, &nested_tag_stem("a", 1, "x")),
        subtree_before,
        "the sub-branch subtree survived the parent replace byte-identically",
    );
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let notes = read.site(1);
    assert_eq!(
        read.read_entry(&notes, &note),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![vs("bye"), None],
        })),
    );
    let tags = read.site(2);
    assert_eq!(
        read.read_entry(&tags, &tag),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![vs("home"), vi(5)],
        })),
        "the preserved sub-branch entry still reads its own record",
    );
}

/// The sub-branch uniform payload-only law for ERASE: a whole erase of a level-1
/// branch entry removes its marker and own field leaves and preserves its sub-branch
/// subtree. The note becomes descendant-only (payload-absent) while its tags subtree
/// survives and reads back.
#[test]
fn an_erase_of_a_branch_entry_removes_its_own_cells_and_preserves_its_sub_branch_subtree() {
    let mut store = nested_store();
    let note = [ks("a"), ki(1)];
    let tag = [ks("a"), ki(1), ks("x")];
    create_at(&mut store, 1, &note, vec![vs("hi"), vs("red")]);
    create_at(&mut store, 2, &tag, vec![vs("home"), vi(5)]);
    let subtree_before = cells_under(&store, &nested_tag_stem("a", 1, "x"));

    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let notes = txn.site(1);
        assert_eq!(
            txn.erase_entry(&notes, &note).expect("erase"),
            EraseOutcome::Erased,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let after = all_cells(&store);
    assert!(
        !after.contains_key(&nested_note_stem("a", 1)),
        "the erase removed the note's own marker",
    );
    assert!(
        !after.contains_key(&physical::stem_field_leaf(
            &nested_note_stem("a", 1),
            branch_field_num(&nested_schema().0, &[0], 0)
        )),
        "the erase removed the note's own field leaf",
    );
    assert_eq!(
        cells_under(&store, &nested_tag_stem("a", 1, "x")),
        subtree_before,
        "the sub-branch subtree survived the parent erase byte-identically",
    );
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let notes = read.site(1);
    assert_eq!(
        read.read_entry(&notes, &note),
        Ok(None),
        "the erased note is now descendant-only (payload-absent)",
    );
    let tags = read.site(2);
    assert_eq!(
        read.read_entry(&tags, &tag),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![vs("home"), vi(5)],
        })),
    );
}

/// The same law at the ROOT over both nested levels: a whole erase of a root book
/// entry removes only its own marker and title and preserves its whole nested subtree
/// (its note and the note's tag). The payload-only law is level-independent, so a root
/// erase never reaches into either descendant level.
#[test]
fn a_root_erase_preserves_the_whole_nested_branch_subtree() {
    let mut store = nested_store();
    let book = [ks("a")];
    let note = [ks("a"), ki(1)];
    let tag = [ks("a"), ki(1), ks("x")];
    create_at(&mut store, 0, &book, vec![vs("Book A")]);
    create_at(&mut store, 1, &note, vec![vs("hi"), None]);
    create_at(&mut store, 2, &tag, vec![vs("home"), None]);
    // The book's subtree below its own payload: the note and tag descendants.
    let note_subtree = cells_under(&store, &nested_note_stem("a", 1));

    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let root = txn.site(0);
        assert_eq!(
            txn.erase_entry(&root, &book).expect("erase"),
            EraseOutcome::Erased,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let after = all_cells(&store);
    assert!(
        !after.contains_key(&nested_book_stem("a")),
        "the root erase removed the book's own marker",
    );
    assert_eq!(
        cells_under(&store, &nested_note_stem("a", 1)),
        note_subtree,
        "both nested levels survived the root erase byte-identically",
    );
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let notes = read.site(1);
    assert_eq!(
        read.read_entry(&notes, &note),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![vs("hi"), None],
        })),
        "the level-1 descendant survived",
    );
    let tags = read.site(2);
    assert_eq!(
        read.read_entry(&tags, &tag),
        Ok(Some(EntryValue {
            groups: Vec::new(),
            fields: vec![vs("home"), None],
        })),
        "the level-2 descendant survived",
    );
}

/// The freeze, `more`, and inclusive-`from` laws hold under the two-component
/// ancestor path `[book, note]`. A link under an absent tag occupies a separate
/// family. Tags under a sibling note lie outside the selected ancestor prefix.
#[test]
fn bounded_acquisition_traverses_a_level_two_layer_and_skips_a_descendant_only_tag() {
    let mut store = nested_store();
    // Present tags t10, t20, t30 under note (a, 1); a descendant-only tag t15 (a
    // links child, no tags marker) between t10 and t20; a decoy tag z under note 2.
    for tag in ["t10", "t20", "t30"] {
        create_at(
            &mut store,
            2,
            &[ks("a"), ki(1), ks(tag)],
            vec![vs("L"), None],
        );
    }
    create_at(
        &mut store,
        3,
        &[ks("a"), ki(1), ks("t15"), ki(1)],
        vec![vs("u")],
    );
    create_at(
        &mut store,
        2,
        &[ks("a"), ki(2), ks("z")],
        vec![vs("L"), None],
    );

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let tags = read.site(2);
    let ancestor = [ks("a"), ki(1)];

    // Freeze the present tags under (a, 1), skipping the descendant-only t15; the
    // (N+1) probe reaches t30 so `more` is set.
    assert_eq!(
        read.iterate_bounded(&tags, &ancestor, None, bound(2)),
        Ok(BoundedKeys {
            keys: vec![ks("t10"), ks("t20")],
            more: true,
        }),
    );
    // A generous bound freezes all three present tags, still skipping t15; the layer
    // is scoped to note 1 so note 2's tag z is not visited.
    assert_eq!(
        read.iterate_bounded(&tags, &ancestor, None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![ks("t10"), ks("t20"), ks("t30")],
            more: false,
        }),
    );
    // Inclusive `from` within the level-2 layer.
    assert_eq!(
        read.iterate_bounded(&tags, &ancestor, Some(ks("t20")), bound(5)),
        Ok(BoundedKeys {
            keys: vec![ks("t20"), ks("t30")],
            more: false,
        }),
    );

    // A different fixed note sees its own tags layer; an absent note sees none.
    let mut read2 = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let tags2 = read2.site(2);
    assert_eq!(
        read2.iterate_bounded(&tags2, &[ks("a"), ki(2)], None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![ks("z")],
            more: false,
        }),
    );
    assert_eq!(
        read2.iterate_bounded(&tags2, &[ks("a"), ki(9)], None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![],
            more: false,
        }),
    );
}

/// A two-column composite-key root addresses each entry by the whole tuple in column
/// order. The columns are the SAME type (both int), so only column *order*
/// distinguishes `[1, 2]` from `[2, 1]`: `node_stem` must split the key-path into the
/// root's two columns and encode them in order, not merely check kinds. A wrong key
/// arity — too few or too many columns — faults corruption at the trust boundary.
#[test]
fn a_composite_key_root_addresses_entries_by_the_ordered_tuple() {
    let mut builder = StoreSchemaBuilder::root("cells", vec![ScalarKind::Int, ScalarKind::Int]);
    builder.scalar_field("v", ScalarKind::Int, true);
    let schema = builder.finish().expect("the composite-key schema builds");
    let sites = vec![SiteTarget::whole_payload(), SiteTarget::field_leaf(0)];
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let entry = txn.site(0);
        txn.create_entry(
            &entry,
            &[ki(1), ki(2)],
            EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(42)))],
            },
        )
        .expect("create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let entry = read.site(0);
    let field = read.site(1);
    // Present at the written tuple; absent at the transposed one — order is load-bearing
    // even with same-typed columns.
    assert_eq!(
        read.presence(&entry, &[ki(1), ki(2)]),
        Ok(Presence::Present)
    );
    assert_eq!(read.presence(&entry, &[ki(2), ki(1)]), Ok(Presence::Absent));
    assert_eq!(
        read.read_field(&field, &[ki(1), ki(2)]),
        Ok(Some(ValueDomain::Scalar(RuntimeScalar::Int(42))))
    );
    // A short or long key-path is a forged arity: corruption, never a mis-split write.
    assert_eq!(
        read.presence(&entry, &[ki(1)]),
        Err(KernelFault::Corruption)
    );
    assert_eq!(
        read.presence(&entry, &[ki(1), ki(2), ki(3)]),
        Err(KernelFault::Corruption)
    );
}

/// A composite-keyed layer is not traversable — the traversal machinery decodes one
/// key per immediate child, so a multi-column traversed layer would mis-read. The
/// verifier parks composite-key traversal; a forged image reaching the kernel with a
/// composite whole-payload site faults at `layer_of` rather than mis-decoding.
#[test]
fn iterate_over_a_composite_keyed_root_layer_faults_corruption() {
    let mut builder = StoreSchemaBuilder::root("cells", vec![ScalarKind::Int, ScalarKind::Int]);
    builder.scalar_field("v", ScalarKind::Int, true);
    let schema = builder.finish().expect("the composite-key schema builds");
    let sites = vec![SiteTarget::whole_payload()];
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let entry = read.site(0);
    // The traversed root layer has two key columns: layer_of's single-column guard
    // faults corruption before any scan.
    assert_eq!(
        read.iterate_bounded(&entry, &[], None, bound(4)),
        Err(KernelFault::Corruption),
    );
}

/// A single-column branch layer beneath a COMPOSITE-keyed root traverses normally: the
/// ancestor key-path locating the parent entry is the root's whole two-column tuple, so
/// `layer_of` consumes it via `take_columns` over multiple ancestor columns.
#[test]
fn iterate_a_single_column_branch_under_a_composite_root_consumes_multi_column_ancestors() {
    let mut builder = StoreSchemaBuilder::root("grid", vec![ScalarKind::Int, ScalarKind::Int]);
    builder.scalar_field("label", ScalarKind::Str, false);
    builder.open_branch("cell", vec![ScalarKind::Int]);
    builder.scalar_field("cval", ScalarKind::Int, true);
    builder.close_branch();
    let schema = builder.finish().expect("the grid schema builds");
    let sites = vec![
        SiteTarget::whole_payload(),
        SiteTarget::branch_entry(Box::from([0u16])),
    ];
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let cell = txn.site(1);
        // Three cells under the composite root entry (1, 2), inserted out of order.
        for c in [3, 1, 2] {
            txn.create_entry(
                &cell,
                &[ki(1), ki(2), ki(c)],
                EntryValue {
                    groups: Vec::new(),
                    fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(0)))],
                },
            )
            .expect("create cell");
        }
        // A cell under a different composite root entry (9, 9), which must not appear.
        txn.create_entry(
            &cell,
            &[ki(9), ki(9), ki(100)],
            EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Int(0)))],
            },
        )
        .expect("create sibling cell");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let cell = read.site(1);
    // Iterate the cell layer under the composite ancestor (1, 2): the two ancestor
    // columns locate the parent entry, and the branch keys come back in order.
    assert_eq!(
        read.iterate_bounded(&cell, &[ki(1), ki(2)], None, bound(10)),
        Ok(BoundedKeys {
            keys: vec![ki(1), ki(2), ki(3)],
            more: false,
        }),
    );
    // A wrong-arity ancestor path (one column where two are needed) faults.
    assert_eq!(
        read.iterate_bounded(&cell, &[ki(1)], None, bound(10)),
        Err(KernelFault::Corruption),
    );
}
