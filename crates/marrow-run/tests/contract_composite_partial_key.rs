//! Partial-key lookups over a composite-identity index return exactly the records
//! under that prefix branch — no shorter sibling, no longer extension of a
//! different key. This pins the prefix-match contract of a composite index read
//! through the runtime: pinning the first identity key narrows to one branch,
//! pinning both is the exact record, and a prefix that matches nothing is empty.

#[macro_use]
mod support;

use support::*;

use marrow_run::Value;
use marrow_store::tree::TreeStore;

/// Memberships keyed by a composite identity `(org, team)`. The index ends with
/// both identity keys, so a partial lookup that pins `org` leaves exactly the
/// identity suffix `(team)` and each entry reconstructs a full `Id(^members)`.
/// Pinning both keys is the exact-identity branch. This is the composite-identity
/// shape: the prefix selects a branch, and the unpinned identity keys distinguish
/// the records inside it.
const MEMBERSHIP_INDEX: &str = "\
resource Member at ^members(org: string, team: string)
    required name: string

    index byOrgTeam(org, team)

pub fn add(org: string, team: string, name: string)
    var m: Member
    m.name = name
    ^members(org, team) = m

pub fn namesInOrg(org: string)
    for id in ^members.byOrgTeam(org)
        print(^members(id).name)

pub fn nameOfTeam(org: string, team: string)
    for id in ^members.byOrgTeam(org, team)
        print(^members(id).name)

pub fn countOrg(org: string): int
    return count(^members.byOrgTeam(org))

pub fn countTeam(org: string, team: string): int
    return count(^members.byOrgTeam(org, team))
";

fn populated_store(program: &marrow_check::CheckedRuntimeProgram) -> TreeStore {
    let store = TreeStore::memory();
    let add = |org: &str, team: &str, name: &str| {
        run_entry(
            &store,
            checked_entry!(
                program,
                "test::add",
                Value::Str(org.into()),
                Value::Str(team.into()),
                Value::Str(name.into())
            ),
        )
        .expect("add member");
    };
    // Two orgs; the first org has two teams, the second reuses a team name from the
    // first. A correct prefix match must distinguish the two `red` teams by org.
    add("acme", "blue", "Cy");
    add("acme", "red", "Ann");
    add("globex", "red", "Dee");
    store
}

#[test]
fn pinning_the_first_key_returns_exactly_that_branch() {
    // `byOrgTeam("acme")` walks both `acme` teams and excludes the `globex` member,
    // even though `globex` shares the `red` team name at the deeper level. Output is
    // in index order over the remaining identity key (team).
    let program = checked_program(MEMBERSHIP_INDEX);
    let store = populated_store(&program);

    let names = run_entry(
        &store,
        checked_entry!(&program, "test::namesInOrg", Value::Str("acme".into())),
    )
    .expect("scan org branch")
    .output;
    assert_eq!(names, "Cy\nAnn\n");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::countOrg", Value::Str("acme".into())),
        )
        .expect("count org")
        .value,
        Some(Value::Int(2)),
        "exactly the two acme members are under the org prefix",
    );
}

#[test]
fn pinning_both_keys_returns_exactly_the_one_record() {
    // `byOrgTeam("acme", "red")` is the full prefix: it excludes the sibling team
    // (`acme`/`blue`) and the other org's `red` team (`globex`/`red`). A longer key
    // sharing a suffix name is not a match.
    let program = checked_program(MEMBERSHIP_INDEX);
    let store = populated_store(&program);

    let names = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::nameOfTeam",
            Value::Str("acme".into()),
            Value::Str("red".into())
        ),
    )
    .expect("scan exact branch")
    .output;
    assert_eq!(names, "Ann\n");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::countTeam",
                Value::Str("acme".into()),
                Value::Str("red".into())
            ),
        )
        .expect("count team")
        .value,
        Some(Value::Int(1)),
    );

    // The other org's identically-named team is its own branch with its own member.
    let globex = run_entry(
        &store,
        checked_entry!(
            &program,
            "test::nameOfTeam",
            Value::Str("globex".into()),
            Value::Str("red".into())
        ),
    )
    .expect("scan other-org team branch")
    .output;
    assert_eq!(globex, "Dee\n");
}

#[test]
fn a_prefix_matching_no_entry_is_empty() {
    // A prefix that no record carries yields nothing and counts zero; the lookup
    // does not fall back to a wider branch.
    let program = checked_program(MEMBERSHIP_INDEX);
    let store = populated_store(&program);

    let names = run_entry(
        &store,
        checked_entry!(&program, "test::namesInOrg", Value::Str("initech".into())),
    )
    .expect("scan absent org")
    .output;
    assert_eq!(names, "");

    assert_eq!(
        run_entry(
            &store,
            checked_entry!(
                &program,
                "test::countTeam",
                Value::Str("acme".into()),
                Value::Str("green".into())
            ),
        )
        .expect("count absent team under a present org")
        .value,
        Some(Value::Int(0)),
        "an absent deeper key is empty even when its org prefix exists",
    );
}
