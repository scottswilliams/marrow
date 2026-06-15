use crate::support;
use std::collections::HashMap;

use marrow_check::CheckedProgram;
use marrow_check::test_support::commit_then_check;

use support::{check_with_accepted, temp_project, write};

/// Map each member of `enum_name` (in `module_name`) to its bound stable catalog id,
/// keyed by the member's leaf name. Panics if a member has no bound id, so a regression
/// to unbound identity fails loudly rather than silently comparing two `None`s as equal.
fn member_catalog_ids(
    program: &CheckedProgram,
    module_name: &str,
    enum_name: &str,
) -> HashMap<String, String> {
    let facts = &program.facts;
    let module = facts.module_id(module_name).expect("module fact");
    let enum_id = facts.enum_id(module, enum_name).expect("enum fact");
    facts
        .enum_members()
        .iter()
        .filter(|member| member.enum_id == enum_id)
        .map(|member| {
            (
                member.name.clone(),
                member
                    .catalog_id
                    .clone()
                    .expect("a committed enum member must carry a stable catalog id"),
            )
        })
        .collect()
}

/// An enum member's durable identity is keyed by the member, not by its position in
/// source. Once a baseline catalog freezes each member's stable id, reordering the
/// members in the declaration must leave every member mapped to the *same* id on a
/// re-check — the stored value is a member identity, so reordering keeps every identity
/// and needs no repair.
///
/// The test commits a baseline, captures each member name's frozen id, rewrites the
/// source with the members in a different order, re-checks through the production
/// pipeline, and asserts the name->id map is unchanged. A binding that keyed identity to
/// source order would hand `active` the id that was `banned`'s, breaking this map.
#[test]
fn enum_member_catalog_ids_are_stable_across_a_source_reorder() {
    let root = temp_project("enum-member-id-reorder", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nenum Status\n    active\n    archived\n    banned\n",
        );
    });

    let committed = commit_then_check(&root).expect("committed fixture");
    let before = member_catalog_ids(&committed, "m", "Status");
    assert_eq!(
        before.len(),
        3,
        "all three members carry a frozen id: {before:?}"
    );

    // Reorder the members; the declaration set is identical, only the order differs.
    write(
        &root,
        "src/m.mw",
        "module m\nenum Status\n    banned\n    active\n    archived\n",
    );
    let (recheck, reordered) = check_with_accepted(&root);
    assert!(!recheck.has_errors(), "{:#?}", recheck.diagnostics);
    let after = member_catalog_ids(&reordered, "m", "Status");

    assert_eq!(
        before, after,
        "each enum member must keep its frozen catalog id across a source reorder"
    );
}

/// Re-checking the *same* committed source is also stable: a member's frozen id reads back
/// unchanged rather than being re-minted. This is the determinism floor the reorder test
/// rests on — without it, a re-check could mint fresh random ids regardless of order, and
/// the reorder comparison would be meaningless.
#[test]
fn enum_member_catalog_ids_survive_an_unchanged_recheck() {
    let root = temp_project("enum-member-id-recheck", |root| {
        write(
            root,
            "src/m.mw",
            "module m\nenum Status\n    active\n    archived\n    banned\n",
        );
    });

    let committed = commit_then_check(&root).expect("committed fixture");
    let before = member_catalog_ids(&committed, "m", "Status");

    let (recheck, rechecked) = check_with_accepted(&root);
    assert!(!recheck.has_errors(), "{:#?}", recheck.diagnostics);
    let after = member_catalog_ids(&rechecked, "m", "Status");

    assert_eq!(
        before, after,
        "a committed enum member id must survive an unchanged re-check"
    );
}
