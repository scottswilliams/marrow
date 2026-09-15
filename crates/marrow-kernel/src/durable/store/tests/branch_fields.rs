//! Field-exact branch operations.

use super::*;

//
// A field-exact set on a branch entry addresses one leaf of a branch node directly
// (`BranchField`). Its engine write is one cell regardless of the branch record's
// width. It checks the branch node's marker at its own stem — never the root's.

/// A wide-record branch schema: root `books` keyed by string with a required
/// `title`, and a branch `notes` keyed by int with a required `text` plus six sparse
/// `f0..f5` fields. The site table addresses the root (0), the branch entry (1), the
/// middle sparse branch field `f2` (2, branch field index 3), and the required branch
/// field `text` (3, branch field index 0).
fn wide_branch_schema() -> (StoreSchema, Vec<SiteTarget>) {
    let mut builder = StoreSchemaBuilder::root("books", vec![ScalarKind::Str]);
    builder.scalar_field("title", ScalarKind::Str, true);
    builder.open_branch("notes", vec![ScalarKind::Int]);
    builder.scalar_field("text", ScalarKind::Str, true);
    for i in 0..6 {
        builder.scalar_field(format!("f{i}"), ScalarKind::Int, false);
    }
    builder.close_branch();
    let schema = builder.finish().expect("the wide branch schema builds");
    let sites = vec![
        SiteTarget::whole_payload(),
        branch_entry(&[0]),
        branch_field(&[0], 3),
        branch_field(&[0], 0),
    ];
    (schema, sites)
}

/// A field-exact set on a present wide-record branch entry writes exactly one new
/// leaf cell, independent of the branch record's width, and leaves every other cell
/// (the marker, the required `text`, and the untouched sparse fields) byte-identical.
/// This is the branch wide-resource evidence: field-exact write work is O(1) plus the
/// node's own incident cells, not proportional to the record width.
#[test]
fn a_field_exact_branch_set_writes_one_leaf_regardless_of_branch_width() {
    let (schema, sites) = wide_branch_schema();
    let mut store = DurableStore::from_engine(MemoryEngine::new(), project(&schema, sites));
    let note = [KeyScalar::Str("a".into()), KeyScalar::Int(7)];

    // Create the branch entry with only its required `text` present.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let branch = txn.site(1);
        let mut fields = vec![Some(ValueDomain::Scalar(RuntimeScalar::Str("hi".into())))];
        fields.extend(std::iter::repeat_n(None, 6));
        txn.create_entry(
            &branch,
            &note,
            EntryValue {
                groups: Vec::new(),
                fields,
            },
        )
        .expect("branch create");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let before = all_cells(&store);

    // A field-exact set of one middle sparse field on the present wide branch entry.
    {
        let mut txn = store
            .txn_session(InvocationGrant::full_store(), write_demand())
            .expect("txn session");
        let f2 = txn.site(2);
        txn.set_field(&f2, &note, ValueDomain::Scalar(RuntimeScalar::Int(42)))
            .expect("field-exact set");
        assert!(matches!(txn.commit(), CommitResult::Committed));
    }
    let after = all_cells(&store);

    assert_eq!(
        after.len(),
        before.len() + 1,
        "a field-exact set on a 7-field branch record writes exactly one new leaf",
    );
    // Every pre-existing cell is byte-identical, except the per-commit witness generation
    // (commit metadata, not application data): the write touched only the one leaf.
    let witness = physical::meta_key(super::super::handle::WITNESS);
    for (key, value) in &before {
        if key == &witness {
            continue;
        }
        assert_eq!(
            after.get(key),
            Some(value),
            "a field-exact set left every prior cell untouched",
        );
    }
}
