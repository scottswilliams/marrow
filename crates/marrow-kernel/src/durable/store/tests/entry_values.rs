//! Managed-index maintenance over whole-entry writes: create, replace and erase each
//! move exactly the index rows their projection is present for.

use super::*;

/// Creating an indexed entry adds exactly its row to every index whose projection is
/// fully present: the non-unique `byLabel` and the unique `byValue`.
#[test]
fn create_adds_a_row_to_every_index() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        assert_eq!(
            txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap(),
            CreateOutcome::Created,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        index_cells(&store),
        sorted(vec![label_cell("a", "x"), value_cell("a", 1)]),
    );
}

/// Changing a projected field moves that index's row and leaves an index the field
/// does not project untouched. Setting `label` from `x` to `y` moves the `byLabel`
/// row; `byValue` (over `value`) is unchanged.
#[test]
fn changing_a_projected_field_moves_only_its_index_row() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        let label = txn.site(2);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        txn.set_field(
            &label,
            &[ks("a")],
            ValueDomain::Scalar(RuntimeScalar::Str("y".into())),
        )
        .unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        index_cells(&store),
        sorted(vec![label_cell("a", "y"), value_cell("a", 1)]),
    );
}

/// Erasing an indexed entry removes exactly its rows and leaves a sibling entry's rows
/// intact — the index analogue of the descendant-preserving erase.
#[test]
fn erasing_one_entry_leaves_a_siblings_rows_intact() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        txn.create_entry(&e, &[ks("b")], ent(2, Some("y"))).unwrap();
        assert_eq!(
            txn.erase_entry(&e, &[ks("a")]).unwrap(),
            EraseOutcome::Erased
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        index_cells(&store),
        sorted(vec![label_cell("b", "y"), value_cell("b", 2)]),
    );
}

/// A clear of a projected sparse field removes that index's row (the entry drops out
/// of `byLabel`) without disturbing an index the field does not project.
#[test]
fn clearing_a_projected_field_removes_its_row() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        let label = txn.site(2);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        txn.erase_field(&label, &[ks("a")]).unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    // Only the unique byValue row survives; byLabel has no row for an absent label.
    assert_eq!(index_cells(&store), sorted(vec![value_cell("a", 1)]));
}

/// A replace rewrites the rows to the new projected values, dropping the old.
#[test]
fn replacing_an_entry_rewrites_its_rows() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        txn.replace_entry(&e, &[ks("a")], ent(9, Some("z")))
            .unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        index_cells(&store),
        sorted(vec![label_cell("a", "z"), value_cell("a", 9)]),
    );
}

/// A second entry colliding on a unique index faults `UniqueIndexViolation`, and the
/// transaction rolls back without poisoning: the committed first entry survives and a
/// fresh transaction still works.
#[test]
fn a_unique_collision_faults_and_rolls_back_without_poisoning() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        // "b" collides with "a" on the unique byValue index (both value 1).
        assert_eq!(
            txn.create_entry(&e, &[ks("b")], ent(1, Some("y"))),
            Err(KernelFault::UniqueIndexViolation),
        );
        // The transaction is dropped without commit: a rollback.
    }
    // Only "a"'s rows remain; "b" never landed.
    assert_eq!(
        index_cells(&store),
        sorted(vec![label_cell("a", "x"), value_cell("a", 1)]),
    );
    // The store is not poisoned: a fresh transaction commits.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        txn.create_entry(&e, &[ks("c")], ent(2, Some("z"))).unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        index_cells(&store),
        sorted(vec![
            label_cell("a", "x"),
            label_cell("c", "z"),
            value_cell("a", 1),
            value_cell("c", 2),
        ]),
    );
}

/// Setting a projected field that was absent adds the index row without removing a
/// non-existent old row (the missing-old case): an entry created without a `label` has
/// no `byLabel` row until the field is set.
#[test]
fn setting_an_absent_projected_field_adds_a_row() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        let label = txn.site(2);
        // Created with no label: only the unique byValue row exists.
        txn.create_entry(&e, &[ks("a")], ent(1, None)).unwrap();
        txn.set_field(
            &label,
            &[ks("a")],
            ValueDomain::Scalar(RuntimeScalar::Str("x".into())),
        )
        .unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        index_cells(&store),
        sorted(vec![label_cell("a", "x"), value_cell("a", 1)]),
    );
}

/// A projected leaf that will not decode is corruption: maintenance reading the old
/// projected state over a tampered store faults `Corruption` rather than trusting an
/// undecodable value into an index key.
#[test]
fn a_corrupt_projected_leaf_faults_corruption() {
    let mut store =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    // Tamper the `label` leaf of entry "a" with bytes no value decodes.
    let marker = physical::marker_key(0, &[ks("a")]);
    let leaf = physical::stem_field_leaf(&marker, field_num(&schema(), 1));
    {
        let mut txn = store.engine.begin().expect("begin");
        // Bytes no value codec decodes, spelled decimal: a structural tag literal
        // belongs to physical.rs alone.
        txn.put(&leaf, vec![255, 255, 255]).expect("put garbage");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    // A field write that maintains byLabel must read the corrupt old value and fault.
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .unwrap();
    let label = txn.site(2);
    assert_eq!(
        txn.set_field(
            &label,
            &[ks("a")],
            ValueDomain::Scalar(RuntimeScalar::Str("y".into())),
        ),
        Err(KernelFault::Corruption),
    );
}

/// The same index cells result over the in-memory and redb engines: maintenance is
/// kernel logic above the byte engine, so the two backends agree cell for cell.
#[test]
fn index_maintenance_agrees_across_engines() {
    fn replay<E: ByteEngine>(store: &mut DurableStore<E>) {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .unwrap();
        let e = txn.site(0);
        let label = txn.site(2);
        txn.create_entry(&e, &[ks("a")], ent(1, Some("x"))).unwrap();
        txn.create_entry(&e, &[ks("b")], ent(2, Some("y"))).unwrap();
        txn.set_field(
            &label,
            &[ks("a")],
            ValueDomain::Scalar(RuntimeScalar::Str("z".into())),
        )
        .unwrap();
        txn.erase_entry(&e, &[ks("b")]).unwrap();
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut mem =
        DurableStore::from_engine(MemoryEngine::new(), project(&indexed_schema(), sites()));
    replay(&mut mem);
    let (mut native, _temp) = native_indexed();
    replay(&mut native);
    assert_eq!(
        index_cells(&mem),
        index_cells(&native),
        "the two engines disagree on maintained index cells",
    );
    assert_eq!(
        index_cells(&mem),
        sorted(vec![label_cell("a", "z"), value_cell("a", 1)])
    );
}

// ---- Durable groups: payload-only, group-scoped ----
//
// These drive the kernel's group ops through directly-constructed group sites. A group
// is part of the entry's payload — no marker, no key — so its whole read follows the
// entry's presence and its whole replace/erase confine to the group's own leaves.

/// A group-bearing root: `books`(Str) with a required `title` and a sparse `summary`,
/// two unkeyed groups `details {pages, language}` and `credits {author}` (all sparse),
/// and a keyed branch `notes(Int){text}`. Sites: 0 whole payload, 1 group `details`,
/// 2 group `credits`, 3 branch `notes` entry.
fn group_schema() -> (StoreSchema, Vec<SiteTarget>) {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.scalar_field("summary", ScalarKind::Str, false);
    builder.open_group("details");
    builder.scalar_field("pages", ScalarKind::Int, false);
    builder.scalar_field("language", ScalarKind::Str, false);
    builder.close_group();
    builder.open_group("credits");
    builder.scalar_field("author", ScalarKind::Str, false);
    builder.close_group();
    builder.open_branch("notes", vec![ScalarKind::Int]);
    builder.scalar_field("text", ScalarKind::Str, true);
    builder.close_branch();
    let schema = builder.finish().expect("the group schema builds");
    let sites = vec![
        SiteTarget::whole_payload(),
        SiteTarget::group_entry(0),
        SiteTarget::group_entry(1),
        SiteTarget::branch_entry(vec![0u16]),
    ];
    (schema, sites)
}

fn group_store() -> DurableStore<MemoryEngine> {
    let (schema, sites) = group_schema();
    DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites))
}

/// A complete `books` entry over the group schema: top-level `title` and `summary`,
/// with both groups supplied and every group leaf vacant.
fn book_with_vacant_groups(title: &str, summary: Option<&str>) -> EntryValue {
    EntryValue {
        fields: vec![vs(title), summary.and_then(vs)],
        groups: vec![
            EntryValue {
                fields: vec![None, None],
                groups: Vec::new(),
            },
            EntryValue {
                fields: vec![None],
                groups: Vec::new(),
            },
        ],
    }
}

/// The physical prefix of book `book`'s `details` group — the byte range its leaves
/// occupy.
fn details_prefix(book: &str) -> Vec<u8> {
    physical::group_stem(
        &physical::marker_key(0, &[ks(book)]),
        group_num(&group_schema().0, 0),
    )
}

/// Book `book`'s own entry cells (its marker is a byte-prefix of its whole subtree)
/// excluding those under `exclude`: the sibling cells a group write must leave
/// byte-identical. Store-wide meta cells (the profile and the per-commit witness) are
/// outside the entry family, so they never enter this comparison.
fn entry_siblings(
    store: &DurableStore<MemoryEngine>,
    book: &str,
    exclude: &[u8],
) -> std::collections::BTreeMap<Vec<u8>, Vec<u8>> {
    let entry = physical::marker_key(0, &[ks(book)]);
    all_cells(store)
        .into_iter()
        .filter(|(key, _)| key.starts_with(&entry) && !key.starts_with(exclude))
        .collect()
}

/// A whole-group write — replace or erase, through the production transaction pipeline
/// — disturbs no sibling cell. Every cell outside the `details` group's own prefix (the
/// entry marker, the entry's top-level fields, the sibling `credits` group's leaves, and
/// a branch note's cells) is byte-identical before and after, while the group itself is
/// exactly replaced (omitted leaves drop) and then erased.
#[test]
fn a_group_write_never_disturbs_siblings() {
    let mut store = group_store();
    let book = [ks("a")];

    // Seed the entry, populate both groups, and add a branch note.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        txn.create_entry(
            &root,
            &book,
            book_with_vacant_groups("Small Gods", Some("a novel")),
        )
        .expect("create root");
        let details = txn.site(1);
        txn.replace_group(
            &details,
            &book,
            EntryValue {
                groups: Vec::new(),
                fields: vec![vi(384), vs("en")],
            },
        )
        .expect("populate details");
        let credits = txn.site(2);
        txn.replace_group(
            &credits,
            &book,
            EntryValue {
                groups: Vec::new(),
                fields: vec![vs("Pratchett")],
            },
        )
        .expect("populate credits");
        let notes = txn.site(3);
        txn.create_entry(
            &notes,
            &[ks("a"), ki(1)],
            EntryValue {
                groups: Vec::new(),
                fields: vec![vs("note one")],
            },
        )
        .expect("create note");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let prefix = details_prefix("a");
    let siblings_before = entry_siblings(&store, "a", &prefix);
    assert!(
        all_cells(&store).keys().any(|k| k.starts_with(&prefix)),
        "the details group has leaves before the write",
    );

    // Replace `details` with a partial value: `pages` changes, `language` omitted.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let details = txn.site(1);
        txn.replace_group(
            &details,
            &book,
            EntryValue {
                groups: Vec::new(),
                fields: vec![vi(999), None],
            },
        )
        .expect("replace details");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        entry_siblings(&store, "a", &prefix),
        siblings_before,
        "a group replace disturbs no sibling cell",
    );

    // The group now reads pages=999 with language dropped — exact replacement.
    {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read");
        let details = read.site(1);
        let value = read
            .read_group(&details, &book)
            .expect("read group")
            .expect("present entry ⇒ present group");
        assert_eq!(value.fields, vec![vi(999), None]);
    }

    // Erase `details`: its leaves go, siblings still byte-identical.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let details = txn.site(1);
        assert_eq!(
            txn.erase_group(&details, &book).expect("erase details"),
            EraseOutcome::Erased,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert_eq!(
        entry_siblings(&store, "a", &prefix),
        siblings_before,
        "a group erase disturbs no sibling cell",
    );
    assert!(
        all_cells(&store).keys().all(|k| !k.starts_with(&prefix)),
        "erase removed every one of the group's leaves",
    );

    // The entry is still present, so the erased group reads present with vacant leaves.
    {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read");
        let details = read.site(1);
        let value = read
            .read_group(&details, &book)
            .expect("read group")
            .expect("present entry ⇒ present group");
        assert_eq!(
            value.fields,
            vec![None, None],
            "a present entry's erased group reads present with vacant leaves",
        );
    }
}

/// A group's whole read follows its containing entry's presence, and a group has no
/// independent existence: over a payload-absent entry the group reads absent and a
/// replace is `Missing` (writing nothing); once the entry is present but the group was
/// never populated, it reads present with vacant leaves.
#[test]
fn read_group_follows_entry_presence_and_replace_requires_the_entry() {
    let mut store = group_store();
    let book = [ks("a")];

    // No entry: the group reads absent, and a replace is a marker/payload mismatch
    // that touches nothing.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let details = txn.site(1);
        assert!(
            txn.read_group(&details, &book).expect("read").is_none(),
            "no entry ⇒ group absent",
        );
        assert_eq!(
            txn.replace_group(
                &details,
                &book,
                EntryValue {
                    groups: Vec::new(),
                    fields: vec![vi(1), vs("en")],
                },
            ),
            Err(KernelFault::Corruption),
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let entry = physical::marker_key(0, &[ks("a")]);
    assert!(
        all_cells(&store).keys().all(|key| !key.starts_with(&entry)),
        "a refused group replace wrote no entry cell",
    );

    // Create the entry; the group now reads present with vacant leaves.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        txn.create_entry(&root, &book, book_with_vacant_groups("t", None))
            .expect("create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read");
        let details = read.site(1);
        let value = read
            .read_group(&details, &book)
            .expect("read")
            .expect("present entry ⇒ present group");
        assert_eq!(
            value.fields,
            vec![None, None],
            "an unpopulated group reads present with vacant leaves",
        );
    }
}

/// A present entry missing a `required` group leaf is a marker/payload mismatch — the
/// whole-group read faults corruption, exactly as a present entry missing a required
/// top-level field does. Defense in depth over the trust boundary: a committed store
/// should never hold this state.
#[test]
fn a_present_entry_missing_a_required_group_leaf_is_corruption() {
    use crate::codec::value::encode_domain;

    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_group("meta");
    builder.scalar_field("isbn", ScalarKind::Str, true);
    builder.close_group();
    let schema = builder.finish().expect("the required-group schema builds");
    let sites = vec![SiteTarget::whole_payload(), SiteTarget::group_entry(0)];

    // Inject a present entry (marker + required title) with the required group leaf
    // `isbn` absent — a state the ops never write, so it is seeded raw.
    let book_stem = physical::marker_key(0, &[ks("a")]);
    let title_bytes =
        encode_domain(&ValueDomain::Scalar(RuntimeScalar::Str("t".into()))).expect("encode title");
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        txn.put(&book_stem, physical::MARKER_VALUE.to_vec())
            .expect("seed marker");
        txn.put(
            &physical::stem_field_leaf(&book_stem, field_num(&group_schema().0, 0)),
            title_bytes,
        )
        .expect("seed title");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let mut store = DurableStore::from_engine(engine, project(&schema, sites));

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read");
    let meta = read.site(1);
    assert!(
        matches!(
            read.read_group(&meta, &[ks("a")]),
            Err(KernelFault::Corruption)
        ),
        "a present entry missing a required group leaf is corruption",
    );
}

/// The whole-entry read is the group-inclusive materialization owner, so it enforces
/// group-leaf required-completeness too: a present entry missing a `required` group leaf
/// faults corruption on the whole-entry read, exactly as the group-scoped read does and
/// exactly as a missing required top-level field does. Defense in depth over the trust
/// boundary — the ops never write this state.
#[test]
fn a_whole_entry_read_faults_on_a_present_entry_missing_a_required_group_leaf() {
    use crate::codec::value::encode_domain;

    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_group("meta");
    builder.scalar_field("isbn", ScalarKind::Str, true);
    builder.close_group();
    let schema = builder.finish().expect("the required-group schema builds");
    let sites = vec![SiteTarget::whole_payload(), SiteTarget::group_entry(0)];

    // Seed a present entry (marker + required title) with the required group leaf
    // `isbn` absent — a state the ops never write.
    let book_stem = physical::marker_key(0, &[ks("a")]);
    let title_bytes =
        encode_domain(&ValueDomain::Scalar(RuntimeScalar::Str("t".into()))).expect("encode title");
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        txn.put(&book_stem, physical::MARKER_VALUE.to_vec())
            .expect("seed marker");
        txn.put(
            &physical::stem_field_leaf(&book_stem, field_num(&group_schema().0, 0)),
            title_bytes,
        )
        .expect("seed title");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let mut store = DurableStore::from_engine(engine, project(&schema, sites));

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read");
    let root = read.site(0);
    assert!(
        matches!(
            read.read_entry(&root, &[ks("a")]),
            Err(KernelFault::Corruption)
        ),
        "a whole-entry read of a present entry missing a required group leaf is corruption",
    );
}

/// A forged cell impersonating a group leaf (the `0x28` group tag) under an entry stem
/// that carries no payload marker is a marker/payload mismatch, not a present group: the
/// group tag never conjures a marker into being. A committed whole-entry read and a
/// committed group read both fail closed with corruption, so a hostile store that seeds a
/// `0x28` cell without its owning marker cannot forge a phantom entry or group.
#[test]
fn a_forged_markerless_group_leaf_cell_reads_as_corruption() {
    use crate::codec::value::encode_domain;

    let (schema, sites) = group_schema();
    // Seed only a `details.pages` group leaf (tag `0x28`) under book "a" — no marker,
    // no other cell. A group leaf is the entry's own payload, so a markerless one is an
    // orphan, never a descendant-only node.
    let book_stem = physical::marker_key(0, &[ks("a")]);
    let group_stem = physical::group_stem(&book_stem, group_num(&group_schema().0, 0));
    let leaf = physical::stem_field_leaf(&group_stem, group_field_num(&group_schema().0, 0, 0));
    let pages_bytes =
        encode_domain(&ValueDomain::Scalar(RuntimeScalar::Int(384))).expect("encode pages");
    let mut engine = MemoryEngine::new();
    {
        let mut txn = engine.begin().expect("begin");
        txn.put(&leaf, pages_bytes).expect("seed forged group leaf");
        assert_eq!(txn.commit(), CommitOutcome::Confirmed);
    }
    let mut store = DurableStore::from_engine(engine, project(&schema, sites));

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read");
    let root = read.site(0);
    assert!(
        matches!(
            read.read_entry(&root, &[ks("a")]),
            Err(KernelFault::Corruption)
        ),
        "a whole-entry read over a forged markerless group leaf is corruption",
    );
    let details = read.site(1);
    assert!(
        matches!(
            read.read_group(&details, &[ks("a")]),
            Err(KernelFault::Corruption)
        ),
        "a group read over a forged markerless group leaf is corruption",
    );
}

/// A whole-entry value carries its groups. A create that supplies the group
/// sub-records writes their leaves as the entry's own payload, and a whole-entry read
/// materializes them back aligned to the schema's groups. A group-bearing entry reads
/// its group through the whole entry as well as the group-scoped op, so
/// `node_write`/`node_cells`/`op_read_entry` are all group-inclusive.
#[test]
fn a_whole_entry_create_writes_and_reads_back_its_groups() {
    let mut store = group_store();
    let book = [ks("a")];
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        txn.create_entry(
            &root,
            &book,
            EntryValue {
                fields: vec![vs("Small Gods"), vs("a novel")],
                groups: vec![
                    EntryValue {
                        fields: vec![vi(384), vs("en")],
                        groups: Vec::new(),
                    },
                    EntryValue {
                        fields: vec![vs("Pratchett")],
                        groups: Vec::new(),
                    },
                ],
            },
        )
        .expect("create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read");
    let root = read.site(0);
    let value = read
        .read_entry(&root, &book)
        .expect("read entry")
        .expect("present");
    assert_eq!(value.fields, vec![vs("Small Gods"), vs("a novel")]);
    assert_eq!(
        value.groups,
        vec![
            EntryValue {
                fields: vec![vi(384), vs("en")],
                groups: Vec::new(),
            },
            EntryValue {
                fields: vec![vs("Pratchett")],
                groups: Vec::new(),
            },
        ],
        "a whole-entry read materializes every group sub-record in schema order",
    );
    // The group-scoped read agrees with the whole-entry materialization.
    let details = read.site(1);
    assert_eq!(
        read.read_group(&details, &book)
            .expect("read group")
            .expect("present")
            .fields,
        vec![vi(384), vs("en")],
    );
}

/// A whole-entry erase sweeps the entry's group leaves along with its marker and
/// top-level fields — a group is the entry's own payload, so no group leaf orphans —
/// while a keyed branch descendant survives the erase (the descendant-preserving law).
#[test]
fn a_whole_entry_erase_sweeps_group_leaves_and_preserves_branches() {
    let mut store = group_store();
    let book = [ks("a")];
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        txn.create_entry(
            &root,
            &book,
            EntryValue {
                fields: vec![vs("t"), None],
                groups: vec![
                    EntryValue {
                        fields: vec![vi(384), vs("en")],
                        groups: Vec::new(),
                    },
                    EntryValue {
                        fields: vec![vs("Pratchett")],
                        groups: Vec::new(),
                    },
                ],
            },
        )
        .expect("create");
        let notes = txn.site(3);
        txn.create_entry(
            &notes,
            &[ks("a"), ki(1)],
            EntryValue {
                fields: vec![vs("note one")],
                groups: Vec::new(),
            },
        )
        .expect("create note");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    assert!(
        all_cells(&store)
            .keys()
            .any(|k| k.starts_with(&details_prefix("a"))),
        "the group has leaves before the erase",
    );

    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        assert_eq!(
            txn.erase_entry(&root, &book).expect("erase entry"),
            EraseOutcome::Erased,
        );
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let entry = physical::marker_key(0, &[ks("a")]);
    let note_stem = physical::marker_key(branch_num(&group_schema().0, &[0]), &[ks("a"), ki(1)]);
    let cells = all_cells(&store);
    assert!(
        cells.keys().all(|k| !k.starts_with(&details_prefix("a"))),
        "the whole-entry erase left no group leaf orphaned",
    );
    assert!(
        !cells.contains_key(&entry),
        "the whole-entry erase removed the marker",
    );
    assert!(
        cells.contains_key(&note_stem),
        "a keyed branch descendant survives the whole-entry erase",
    );
}

/// A whole-entry replace is exact over the entry's groups too: an omitted sparse group
/// leaf does not survive the replacement, matching the top-level field
/// exact-replacement law, while keyed branch descendants are preserved.
#[test]
fn a_whole_entry_replace_drops_omitted_group_leaves() {
    let mut store = group_store();
    let book = [ks("a")];
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        txn.create_entry(
            &root,
            &book,
            EntryValue {
                fields: vec![vs("t"), vs("full")],
                groups: vec![
                    EntryValue {
                        fields: vec![vi(384), vs("en")],
                        groups: Vec::new(),
                    },
                    EntryValue {
                        fields: vec![vs("Pratchett")],
                        groups: Vec::new(),
                    },
                ],
            },
        )
        .expect("create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    // Replace the whole entry: `details.language` and `credits.author` omitted.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn");
        let root = txn.site(0);
        txn.replace_entry(
            &root,
            &book,
            EntryValue {
                fields: vec![vs("t"), None],
                groups: vec![
                    EntryValue {
                        fields: vec![vi(999), None],
                        groups: Vec::new(),
                    },
                    EntryValue {
                        fields: vec![None],
                        groups: Vec::new(),
                    },
                ],
            },
        )
        .expect("replace");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }

    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read");
    let root = read.site(0);
    let value = read
        .read_entry(&root, &book)
        .expect("read")
        .expect("present");
    assert_eq!(
        value.groups,
        vec![
            EntryValue {
                fields: vec![vi(999), None],
                groups: Vec::new(),
            },
            EntryValue {
                fields: vec![None],
                groups: Vec::new(),
            },
        ],
        "an omitted group leaf does not survive a whole-entry replace",
    );
}
