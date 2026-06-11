//! Apply over a checked retire: an index retire clears derived cells with no approval,
//! while a destructive member retire requires the maintenance gate and a scoped
//! approval whose populated count matches the witness per id. Mismatched, swapped, or
//! ungated approvals abort without dropping data, completion rejects a per-id receipt
//! count moved between ids, and a nested-group retire fails closed.

mod evolution_apply_support;

use evolution_apply_support::*;

use marrow_run::evolution::{ApplyError, Approval, apply, verify_activation_completion};
use marrow_store::cell::CatalogId;
use marrow_store::key::SavedKey;
use marrow_store::tree::{DataPathSegment, TreeStore};
use marrow_store::value::{Scalar, encode_value};

/// An explicit `retire` of a store index deletes its derived cells, exactly as a bare
/// source-drop does. An index holds no per-record source data, so retiring one is the
/// same durable operation as dropping it: delete the index-cell subtree under its id.
/// The retire intent must not route to the per-record member-deletion path, which would
/// leave the real index cells orphaned. Apply needs no destructive approval because no
/// source data moves; only derived cells are cleared, and the base records survive.
#[test]
fn explicit_index_retire_deletes_index_cells() {
    let root = temp_project("apply-index-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required isbn: string\n\
             store ^books(id: int): Book\n\
             \x20   index byIsbn(isbn) unique\n\
             pub fn add(isbn: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let index_id = CatalogId::new(index_catalog_id(&accepted_place, "byIsbn")).unwrap();
    let store_id = store_id_of(&accepted_place);

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "isbn", Scalar::Str("111".into()));
    seed.record(2);
    seed.member(2, "isbn", Scalar::Str("222".into()));
    for (key, id) in [("111", 1), ("222", 2)] {
        store
            .write_index_entry(
                &index_id,
                &[SavedKey::Str(key.into())],
                &[SavedKey::Int(id)],
                Vec::new(),
            )
            .expect("seed index entry");
    }
    assert!(
        index_has_children(&store, &index_id),
        "the index starts with cells"
    );

    // Drop the index from source and declare an explicit retire of it.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required isbn: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire ^books.byIsbn\n\
         pub fn add(isbn: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);

    let w = witness(&program, &store);
    // The index retire clears derived cells only, so it stays activatable with no
    // approval, just like a bare source-drop of the index.
    assert!(
        w.is_activatable(),
        "an index retire must not require a destructive approval"
    );
    let outcome = apply(&w, &program, &store, false, None).expect("apply succeeds");

    assert_eq!(outcome.receipt.records_retired, 0);
    assert!(
        !index_has_children(&store, &index_id),
        "an explicit index retire must leave no index cells"
    );
    for (id, isbn) in [(1, "111"), (2, "222")] {
        let bytes = store
            .read_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(
                    CatalogId::new(member_catalog_id(&accepted_place, "isbn")).unwrap(),
                )],
            )
            .expect("read isbn")
            .expect("isbn present");
        assert_eq!(bytes, encode_value(&Scalar::Str(isbn.into())).unwrap());
    }
}

/// A retire over populated data needs maintenance plus a scoped approval. With no
/// approval apply refuses and the witness is non-activatable, leaving the data in
/// place; with maintenance and a matching scoped approval it drops the retired member
/// subtree from every record and stamps the retire receipt.
#[test]
fn destructive_retire_aborts_without_approval_and_deletes_with_matching_approval() {
    let (_root, program, place, store, subtitle_id) =
        destructive_retire_fixture("apply-retire-approval");
    let witness = witness(&program, &store);
    let store_id = store_id_of(&place);
    let subtitle_present = |id: i64| {
        store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(id)],
                &[DataPathSegment::Member(
                    CatalogId::new(subtitle_id.clone()).unwrap(),
                )],
            )
            .expect("exists")
    };

    // No approval: the witness is non-activatable and apply refuses without dropping
    // any data.
    assert!(!witness.is_activatable());
    let result = apply(&witness, &program, &store, true, None);
    assert!(
        matches!(result, Err(ApplyError::ApprovalRequired { .. })),
        "expected ApprovalRequired, got {result:#?}"
    );
    assert!(
        subtitle_present(1),
        "retire without approval must not drop data"
    );

    // Maintenance plus a matching scoped approval drops the member subtree from both
    // records and stamps the retire receipt.
    let approval = Approval {
        retires: vec![(CatalogId::new(subtitle_id.clone()).unwrap(), 2)],
    };
    let outcome = apply(&witness, &program, &store, true, Some(&approval)).expect("apply");
    assert_eq!(outcome.receipt.records_retired, 2);
    for id in [1, 2] {
        assert!(
            !subtitle_present(id),
            "approved retire drops the member subtree"
        );
    }
}

#[test]
fn completion_rejects_retire_receipt_count_moved_between_ids() {
    let root = temp_project("completion-retire-per-id-drift", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   notes: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    let notes_id = member_catalog_id(&accepted_place, "notes");
    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    seed.record(1);
    seed.member(1, "title", Scalar::Str("Dune".into()));
    seed.member_by_id(1, &subtitle_id, Scalar::Str("sub-1".into()));
    seed.member_by_id(1, &notes_id, Scalar::Str("note-1".into()));
    seed.record(2);
    seed.member(2, "title", Scalar::Str("Hyperion".into()));
    seed.member_by_id(2, &subtitle_id, Scalar::Str("sub-2".into()));
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         \x20   retire Book.notes\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let approval = Approval {
        retires: vec![
            (CatalogId::new(subtitle_id.clone()).unwrap(), 2),
            (CatalogId::new(notes_id.clone()).unwrap(), 1),
        ],
    };
    apply(
        &witness(&program, &store),
        &program,
        &store,
        true,
        Some(&approval),
    )
    .expect("apply");

    let mut commit = store
        .read_commit_metadata()
        .expect("read commit")
        .expect("activation commit");
    commit.activation_records_retired_by_id = vec![
        (CatalogId::new(notes_id).unwrap(), 0),
        (CatalogId::new(subtitle_id).unwrap(), 3),
    ];
    store
        .write_commit_metadata(&commit)
        .expect("forge retire receipt counts");
    let error = verify_activation_completion(&program, &store, &commit)
        .expect_err("forged retire receipt fails");

    assert_eq!(error, ApplyError::Drift);
}

/// A multi-retire approval is matched per id: each approved count must equal the
/// witness count for that exact id. A single member whose approved count drifts from
/// the witness is refused, and when two retired members hold different populated counts,
/// swapping the counts between them keeps the sum identical but is out of scope, so apply
/// must still refuse. A merely summed check would bless the swap and drop data the
/// developer did not approve at that scope.
#[test]
fn destructive_multi_retire_approval_is_matched_per_id() {
    let root = temp_project("apply-retire-multi-count", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             \x20   subtitle: string\n\
             \x20   notes: string\n\
             store ^books(id: int): Book\n\
             pub fn add(title: string): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let subtitle_id = member_catalog_id(&accepted_place, "subtitle");
    let notes_id = member_catalog_id(&accepted_place, "notes");

    let store = TreeStore::memory();
    let seed = Seed {
        store: &store,
        place: &accepted_place,
    };
    // Asymmetric populations: subtitle on two records, notes on one. The witness records
    // subtitle = 2 and notes = 1, so a swapped approval (subtitle = 1, notes = 2) sums to
    // the same 3 yet is out of scope per id.
    for id in [1, 2] {
        seed.record(id);
        seed.member(id, "title", Scalar::Str(format!("title-{id}")));
        seed.member_by_id(id, &subtitle_id, Scalar::Str(format!("sub-{id}")));
    }
    seed.member_by_id(1, &notes_id, Scalar::Str("note".into()));

    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   required title: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.subtitle\n\
         \x20   retire Book.notes\n\
         pub fn add(title: string): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);
    let witness = witness(&program, &store);
    let store_id = store_id_of(&accepted_place);
    let both_present = |label: &str| {
        for member_id in [&subtitle_id, &notes_id] {
            assert!(
                store
                    .data_subtree_exists(
                        &store_id,
                        &[SavedKey::Int(1)],
                        &[DataPathSegment::Member(
                            CatalogId::new(member_id.clone()).unwrap(),
                        )],
                    )
                    .expect("exists"),
                "{label} must not drop data"
            );
        }
    };

    // A single member whose approved count drifts below the witness (subtitle approved
    // as 1 where the witness recorded 2, notes correct) is refused without dropping data.
    let single_drift = Approval {
        retires: vec![
            (CatalogId::new(subtitle_id.clone()).unwrap(), 1),
            (CatalogId::new(notes_id.clone()).unwrap(), 1),
        ],
    };
    let result = apply(&witness, &program, &store, true, Some(&single_drift));
    assert!(
        matches!(result, Err(ApplyError::ApprovalMismatch)),
        "a single-member count drift must be rejected, got {result:#?}"
    );
    both_present("a count-drifted approval");

    // Swapping the two counts keeps the sum at 3 but is per-id out of scope, so apply
    // must refuse rather than bless the sum and drop unapproved data.
    let swapped = Approval {
        retires: vec![
            (CatalogId::new(subtitle_id.clone()).unwrap(), 1),
            (CatalogId::new(notes_id.clone()).unwrap(), 2),
        ],
    };
    let result = apply(&witness, &program, &store, true, Some(&swapped));
    assert!(
        matches!(result, Err(ApplyError::ApprovalMismatch)),
        "a per-id-wrong approval with a matching sum must be rejected, got {result:#?}"
    );
    both_present("a per-id-wrong approval");

    // The correctly scoped per-id approval activates and drops both members.
    let scoped = Approval {
        retires: vec![
            (CatalogId::new(subtitle_id.clone()).unwrap(), 2),
            (CatalogId::new(notes_id.clone()).unwrap(), 1),
        ],
    };
    let outcome = apply(&witness, &program, &store, true, Some(&scoped)).expect("apply");
    assert_eq!(outcome.receipt.records_retired, 3);
}

/// Maintenance off blocks a destructive retire even with a matching approval: a hard
/// drop requires the maintenance gate.
#[test]
fn destructive_retire_requires_maintenance() {
    let (root, _program, _place, store, subtitle_id) =
        destructive_retire_fixture("apply-retire-no-maint");
    let program = checked(&root);
    let place = root_place(&program, "books");
    let witness = witness(&program, &store);
    let approval = Approval {
        retires: vec![(CatalogId::new(subtitle_id.clone()).unwrap(), 2)],
    };
    let result = apply(&witness, &program, &store, false, Some(&approval));
    assert!(
        matches!(result, Err(ApplyError::MaintenanceRequired)),
        "expected MaintenanceRequired, got {result:#?}"
    );
    let store_id = store_id_of(&place);
    assert!(
        store
            .data_subtree_exists(
                &store_id,
                &[SavedKey::Int(1)],
                &[DataPathSegment::Member(
                    CatalogId::new(subtitle_id).unwrap()
                )]
            )
            .expect("exists")
    );
}

/// Retiring a member nested under an unkeyed group fails closed: apply does not yet
/// descend a group to drop nested cells, so a nested retire must be non-activatable
/// rather than counting zero populated cells and silently dropping nothing. The records
/// carry `meta.note` cells that a top-level retire path would never reach.
#[test]
fn nested_group_retire_fails_closed() {
    let root = temp_project("apply-nested-retire", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   meta\n\
             \x20       required note: string\n\
             \x20       keep: string\n\
             store ^books(id: int): Book\n\
             pub fn add(): Id(^books)\n\
             \x20   return nextId(^books)\n",
        );
    });
    // Commit the schema that declares `meta.note`, so the nested leaf binds a stable id.
    let accepted = commit_then_check(&root);
    let accepted_place = root_place(&accepted, "books");
    let meta_id = group_member_catalog_id(&accepted_place, "meta");
    let note_id = nested_member_catalog_id(&accepted_place, "meta", "note");

    let store = TreeStore::memory();
    let store_id = store_id_of(&accepted_place);
    // Seed two records each carrying a `meta.note` cell at the nested member path.
    for id in [1, 2] {
        store
            .write_node(&store_id, &[SavedKey::Int(id)])
            .expect("write node");
        store
            .write_data_value(
                &store_id,
                &[SavedKey::Int(id)],
                &[
                    DataPathSegment::Member(CatalogId::new(meta_id.clone()).unwrap()),
                    DataPathSegment::Member(CatalogId::new(note_id.clone()).unwrap()),
                ],
                encode_value(&Scalar::Str(format!("note-{id}"))).expect("encode"),
            )
            .expect("write nested member");
    }

    // Retire the nested leaf; the accepted catalog still names it.
    write(
        &root,
        "src/books.mw",
        "module books\n\
         resource Book\n\
         \x20   meta\n\
         \x20       keep: string\n\
         store ^books(id: int): Book\n\
         evolve\n\
         \x20   retire Book.meta.note\n\
         pub fn add(): Id(^books)\n\
         \x20   return nextId(^books)\n",
    );
    let program = checked(&root);

    let w = witness(&program, &store);
    assert!(
        !w.is_activatable(),
        "a nested retire must not be activatable: {w:#?}"
    );

    // Even under the maintenance gate with an approval, apply must refuse rather than
    // stamp success while the nested cells survive.
    let approval = Approval {
        retires: vec![(CatalogId::new(note_id.clone()).unwrap(), 0)],
    };
    let result = apply(&w, &program, &store, true, Some(&approval));
    assert!(
        matches!(result, Err(ApplyError::NotActivatable)),
        "expected NotActivatable, got {result:#?}"
    );

    // The nested cells are untouched and no stamp landed.
    for id in [1, 2] {
        assert!(
            store
                .data_subtree_exists(
                    &store_id,
                    &[SavedKey::Int(id)],
                    &[
                        DataPathSegment::Member(CatalogId::new(meta_id.clone()).unwrap()),
                        DataPathSegment::Member(CatalogId::new(note_id.clone()).unwrap()),
                    ],
                )
                .expect("exists"),
            "a refused nested retire must not drop the nested cell"
        );
    }
    assert_eq!(
        store.read_commit_metadata().expect("read"),
        None,
        "no stamp"
    );
}
