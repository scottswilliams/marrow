//! The bounded acquisition law: the freeze-then-run kernel primitive.

use super::*;

//
// `iterate_bounded` freezes the first N immediate keys of a durable layer and
// reports whether an (N+1)th existed. It is the bounded, cursor-free acquisition
// the `for … at most N … on more` form runs over: the keys are captured up front
// (so loop-body writes cannot change the frozen set), a descendant-only child is
// skipped by one prefix-successor seek, and an inclusive `from` bounds the start.

/// Create `names` as present root entries (a required `value`, no `label`) in one
/// committed transaction over the flat `counters` schema.
fn seed_root(store: &mut DurableStore<MemoryEngine>, names: &[&str]) {
    let mut txn = store
        .txn_session(InvocationGrant::full_store(), write_demand())
        .expect("txn session");
    let entry = txn.site(0);
    for name in names {
        txn.create_entry(&entry, &[KeyScalar::Str((*name).into())], value_entry(1))
            .expect("create");
    }
    assert!(matches!(txn.commit(), CommitResult::Committed));
}

/// Freeze up to `n` root keys of the `counters` store, starting inclusively at
/// `from` when given.
fn freeze_root(store: &mut DurableStore<MemoryEngine>, from: Option<&str>, n: u32) -> BoundedKeys {
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    read.iterate_bounded(&root, &[], from.map(|s| KeyScalar::Str(s.into())), bound(n))
        .expect("iterate")
}

fn strs(names: &[&str]) -> Vec<KeyScalar> {
    names.iter().map(|s| KeyScalar::Str((*s).into())).collect()
}

/// The freeze law: the frozen set is the first N present keys in ascending order,
/// and `more` is set exactly when an (N+1)th key exists — regardless of insertion
/// order.
#[test]
fn bounded_acquisition_freezes_the_first_n_and_flags_a_further_key() {
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    seed_root(&mut store, &["c", "a", "e", "b", "d"]); // inserted out of order

    // N below the population: the first N, ascending, with `more` set.
    assert_eq!(
        freeze_root(&mut store, None, 3),
        BoundedKeys {
            keys: strs(&["a", "b", "c"]),
            more: true,
        },
    );
    // N equal to the population: every key, `more` clear (no (N+1)th exists).
    assert_eq!(
        freeze_root(&mut store, None, 5),
        BoundedKeys {
            keys: strs(&["a", "b", "c", "d", "e"]),
            more: false,
        },
    );
    // N above the population: every key, `more` clear.
    assert_eq!(
        freeze_root(&mut store, None, 9),
        BoundedKeys {
            keys: strs(&["a", "b", "c", "d", "e"]),
            more: false,
        },
    );
}

/// The 0/1/N/N+1 boundary of the population against a fixed bound N=2.
#[test]
fn bounded_acquisition_covers_the_population_boundary() {
    // 0 present: empty frozen set, no more.
    let mut empty = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    assert_eq!(
        freeze_root(&mut empty, None, 2),
        BoundedKeys {
            keys: vec![],
            more: false,
        },
    );

    // 1 present (< N): the one key, no more.
    let mut one = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    seed_root(&mut one, &["a"]);
    assert_eq!(
        freeze_root(&mut one, None, 2),
        BoundedKeys {
            keys: strs(&["a"]),
            more: false,
        },
    );

    // Exactly N present: both keys, no more (the (N+1)th does not exist).
    let mut exact = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    seed_root(&mut exact, &["a", "b"]);
    assert_eq!(
        freeze_root(&mut exact, None, 2),
        BoundedKeys {
            keys: strs(&["a", "b"]),
            more: false,
        },
    );

    // N+1 present: the first N frozen, `more` set (the third is probed, not frozen).
    let mut over = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    seed_root(&mut over, &["a", "b", "c"]);
    assert_eq!(
        freeze_root(&mut over, None, 2),
        BoundedKeys {
            keys: strs(&["a", "b"]),
            more: true,
        },
    );
}

/// The inclusive `from` lower bound: the walk begins at `from` when present, else at
/// the first present key above it, and is otherwise frozen and flagged as usual.
#[test]
fn bounded_acquisition_from_is_an_inclusive_lower_bound() {
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema(), sites()));
    seed_root(&mut store, &["a", "c", "e"]);

    // `from` a present key: inclusive — the frozen set starts at it.
    assert_eq!(
        freeze_root(&mut store, Some("c"), 5),
        BoundedKeys {
            keys: strs(&["c", "e"]),
            more: false,
        },
    );
    // `from` between two keys: starts at the first present key above it.
    assert_eq!(
        freeze_root(&mut store, Some("b"), 5),
        BoundedKeys {
            keys: strs(&["c", "e"]),
            more: false,
        },
    );
    // `from` at the least key: inclusive, the whole layer.
    assert_eq!(
        freeze_root(&mut store, Some("a"), 5),
        BoundedKeys {
            keys: strs(&["a", "c", "e"]),
            more: false,
        },
    );
    // `from` above every key: empty.
    assert_eq!(
        freeze_root(&mut store, Some("z"), 5),
        BoundedKeys {
            keys: vec![],
            more: false,
        },
    );
    // `from` combines with the bound: the (N+1)th key above `from` sets `more`.
    assert_eq!(
        freeze_root(&mut store, Some("c"), 1),
        BoundedKeys {
            keys: strs(&["c"]),
            more: true,
        },
    );
}

/// Descendant-only entries — markerless roots carrying only a keyed branch child —
/// are skipped by the bounded walk with one prefix-successor seek per run, so the
/// frozen set holds only payload-bearing roots, and the (N+1) probe skips a
/// descendant-only run to reach a real key.
#[test]
fn bounded_acquisition_skips_descendant_only_entries() {
    let mut cells = Vec::new();
    for present in ["k1", "k4"] {
        let stem = book_stem(present);
        cells.push((stem.clone(), physical::MARKER_VALUE.to_vec()));
        cells.push((
            physical::stem_field_leaf(&stem, field_num(&branch_schema().0, 0)),
            b"T".to_vec(),
        ));
    }
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
    let k = |s: &str| KeyScalar::Str(s.into());
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);

    // A generous bound freezes only the two present roots, skipping the two
    // descendant-only entries between them.
    assert_eq!(
        read.iterate_bounded(&root, &[], None, bound(10)),
        Ok(BoundedKeys {
            keys: vec![k("k1"), k("k4")],
            more: false,
        }),
    );
    // With N=1 the (N+1) probe skips the descendant-only run k2,k3 to reach k4, so
    // `more` is set although the two intervening entries carry no payload.
    assert_eq!(
        read.iterate_bounded(&root, &[], None, bound(1)),
        Ok(BoundedKeys {
            keys: vec![k("k1")],
            more: true,
        }),
    );
}

/// Bounded work over fan-out: a present root with a large branch subtree is passed
/// by one prefix-successor seek to reach the next root, so root-layer freezing never
/// reads the subtree — the frozen set is the roots, not their descendants.
#[test]
fn bounded_acquisition_skips_a_large_descendant_fan_out_in_one_seek() {
    let (schema, sites) = branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let root = txn.site(0);
        let branch = txn.site(1);
        for book in ["a", "b"] {
            let title = EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("T".into())))],
            };
            txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                .expect("root create");
        }
        // A large branch fan-out under book "a" the root walk must skip wholesale.
        for note in 0..200i64 {
            let text = EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
            };
            txn.create_entry(
                &branch,
                &[KeyScalar::Str("a".into()), KeyScalar::Int(note)],
                text,
            )
            .expect("note create");
        }
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    let k = |s: &str| KeyScalar::Str(s.into());

    // The root layer freezes only the two book roots; book "a"'s 200-note subtree
    // is skipped in one seek to reach "b".
    assert_eq!(
        read.iterate_bounded(&root, &[], None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![k("a"), k("b")],
            more: false,
        }),
    );
    // With N=1, "b" is the (N+1) probe reached past "a"'s whole fan-out.
    assert_eq!(
        read.iterate_bounded(&root, &[], None, bound(1)),
        Ok(BoundedKeys {
            keys: vec![k("a")],
            more: true,
        }),
    );
}

/// Branch-layer traversal: freezing the immediate keys of a keyed branch beneath a
/// fixed root entry. The frozen set is that branch's own keys, scoped to the given
/// root key (a sibling root's branch of the same name is not visited), with the same
/// freeze / `more` / inclusive-`from` law as the root layer, one level down.
#[test]
fn bounded_acquisition_traverses_a_branch_layer_under_a_fixed_root_key() {
    let (schema, sites) = branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let root = txn.site(0);
        let branch = txn.site(1);
        for book in ["a", "b"] {
            let title = EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("T".into())))],
            };
            txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                .expect("root create");
        }
        // Notes 10,20,30 under "a"; a decoy note 5 under sibling root "b".
        for note in [10i64, 20, 30] {
            let text = EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
            };
            txn.create_entry(
                &branch,
                &[KeyScalar::Str("a".into()), KeyScalar::Int(note)],
                text,
            )
            .expect("note create");
        }
        let decoy = EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("x".into())))],
        };
        txn.create_entry(
            &branch,
            &[KeyScalar::Str("b".into()), KeyScalar::Int(5)],
            decoy,
        )
        .expect("decoy create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let branch = read.site(1);
    let a = [KeyScalar::Str("a".into())];
    let int = KeyScalar::Int;

    // Freeze the notes under "a": bounded and scoped, with `more` when an (N+1)th
    // note exists.
    assert_eq!(
        read.iterate_bounded(&branch, &a, None, bound(2)),
        Ok(BoundedKeys {
            keys: vec![int(10), int(20)],
            more: true,
        }),
    );
    assert_eq!(
        read.iterate_bounded(&branch, &a, None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![int(10), int(20), int(30)],
            more: false,
        }),
        "the branch layer is scoped to root a — b's note key 5 is not visited",
    );
    // Inclusive `from` within the branch layer.
    assert_eq!(
        read.iterate_bounded(&branch, &a, Some(int(20)), bound(5)),
        Ok(BoundedKeys {
            keys: vec![int(20), int(30)],
            more: false,
        }),
    );

    // A different fixed root key sees its own branch layer; an absent root key none.
    let mut read2 = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let branch2 = read2.site(1);
    assert_eq!(
        read2.iterate_bounded(&branch2, &[KeyScalar::Str("b".into())], None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![int(5)],
            more: false,
        }),
    );
    assert_eq!(
        read2.iterate_bounded(&branch2, &[KeyScalar::Str("c".into())], None, bound(5)),
        Ok(BoundedKeys {
            keys: vec![],
            more: false,
        }),
    );
}

/// The family-populated probe over a keyed branch family (the `notes` layer under one
/// book): `Present` when the book has at least one note, `Absent` when it has none or
/// is itself absent — the "does this asset have notes?" question. The probe reads
/// the branch layer scoped to the fixed parent key, so one book's notes never make a
/// sibling's family read populated.
#[test]
fn family_populated_answers_whether_a_branch_family_has_a_child() {
    let (schema, sites) = branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let root = txn.site(0);
        let branch = txn.site(1);
        for book in ["a", "b"] {
            let title = EntryValue {
                groups: Vec::new(),
                fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("T".into())))],
            };
            txn.create_entry(&root, &[KeyScalar::Str(book.into())], title)
                .expect("root create");
        }
        // Only book "a" gets a note; book "b" stays note-less.
        let text = EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
        };
        txn.create_entry(
            &branch,
            &[KeyScalar::Str("a".into()), KeyScalar::Int(1)],
            text,
        )
        .expect("note create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    let branch = read.site(1);
    // The root family is populated (two books exist).
    assert_eq!(read.family_populated(&root, &[]), Ok(Presence::Present));
    // Book "a" has a note; book "b" and an absent book "c" have none.
    assert_eq!(
        read.family_populated(&branch, &[KeyScalar::Str("a".into())]),
        Ok(Presence::Present),
    );
    assert_eq!(
        read.family_populated(&branch, &[KeyScalar::Str("b".into())]),
        Ok(Presence::Absent),
    );
    assert_eq!(
        read.family_populated(&branch, &[KeyScalar::Str("c".into())]),
        Ok(Presence::Absent),
    );
}

/// Entries in a different family do not populate an absent ancestor's family.
/// Both that family and an entirely empty root family read `Absent`.
#[test]
fn family_populated_skips_descendant_only_children_and_empty_families() {
    let (schema, sites) = branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    // A fresh store: the root family is empty.
    {
        let mut read = store
            .read_session(InvocationGrant::full_store(), read_demand())
            .expect("read session");
        let root = read.site(0);
        assert_eq!(read.family_populated(&root, &[]), Ok(Presence::Absent));
    }
    // Give book "a" a note but never a payload marker of its own: "a" is a
    // descendant-only child of the root family. The root family must still read
    // `Absent` — it holds no payload-bearing book.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let branch = txn.site(1);
        let text = EntryValue {
            groups: Vec::new(),
            fields: vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("n".into())))],
        };
        txn.create_entry(
            &branch,
            &[KeyScalar::Str("a".into()), KeyScalar::Int(1)],
            text,
        )
        .expect("note create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let root = read.site(0);
    let branch = read.site(1);
    // "a" has no own payload marker, only a note beneath it.
    assert_eq!(
        read.presence(&root, &[KeyScalar::Str("a".into())]),
        Ok(Presence::Absent)
    );
    // So the root family is not populated, but "a"'s own notes family is.
    assert_eq!(read.family_populated(&root, &[]), Ok(Presence::Absent));
    assert_eq!(
        read.family_populated(&branch, &[KeyScalar::Str("a".into())]),
        Ok(Presence::Present),
    );
}

/// `layer_of`'s hard backstop over the trust boundary (matching `node_stem`): a branch
/// layer's ancestor key-path must be the root key then one key per parent hop. A wrong
/// ancestor arity or a wrong ancestor key kind faults `Corruption` rather than
/// mis-layering the traversal to the root entry family (which would leak the wrong
/// layer's keys). The verifier proves the arity and kinds, so this is the release
/// backstop a forged image cannot slip past.
#[test]
fn a_branch_layer_traversal_with_a_wrong_ancestor_key_path_faults() {
    let (schema, sites) = branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    let mut read = store
        .read_session(InvocationGrant::full_store(), read_demand())
        .expect("read session");
    let branch = read.site(1); // a single-level branch site (needs `[root_key]`)

    // Empty ancestor path: a branch layer needs one ancestor key; zero is a wrong
    // arity that must fault rather than mis-layer to the root's own entry family.
    assert_eq!(
        read.iterate_bounded(&branch, &[], None, bound(4)),
        Err(KernelFault::Corruption),
    );
    // Two ancestor keys where the single-level branch layer needs one: wrong arity.
    assert_eq!(
        read.iterate_bounded(
            &branch,
            &[KeyScalar::Str("a".into()), KeyScalar::Str("b".into())],
            None,
            bound(4),
        ),
        Err(KernelFault::Corruption),
    );
    // Right arity, wrong ancestor kind: the root key is a string, so an int ancestor
    // key is a scalar-kind mismatch at the trust boundary.
    assert_eq!(
        read.iterate_bounded(&branch, &[KeyScalar::Int(0)], None, bound(4)),
        Err(KernelFault::Corruption),
    );
}
