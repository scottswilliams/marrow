//! Enum runtime behavior: saved round-trip and indexing through the checked
//! catalog, nominal equality, `match` arm dispatch by the scrutinee enum, and
//! `is` subtree / leaf membership over nested and duplicated members.

use crate::support;
use support::*;

use marrow_run::Value;
use marrow_store::key::SavedKey;
use marrow_store::tree::TreeStore;
use marrow_store::tree::decode_tree_enum_member;
use marrow_store::value::{ScalarType, decode_value};

/// member. The test observes the language behavior instead of the storage codec.
#[test]
fn an_enum_field_round_trips_through_saved_data() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n\n\
         pub fn seed(id: int)\n    ^orders(id).state = Status::archived\n\n\
         pub fn matches_archived(id: int): bool\n    return (^orders(id).state ?? Status::active) == Status::archived\n",
    );
    let store = TreeStore::memory();
    run_entry(
        &store,
        checked_entry!(&program, "test::seed", Value::Int(7)),
    )
    .expect("seed");
    let bytes = read_data_bytes(
        &program,
        &store,
        "orders",
        &[SavedKey::Int(7)],
        &data_path(&program, "orders", &["state"]),
    )
    .expect("enum leaf is written");
    assert!(
        decode_value(&bytes, ScalarType::Int).is_none(),
        "enum leaves must not use the old scalar ordinal codec"
    );
    let stored = decode_tree_enum_member(&bytes).expect("enum leaf uses tree enum member codec");
    assert_eq!(stored.enum_id(), &enum_catalog_id(&program, "Status"));
    assert_eq!(
        stored.member_id(),
        &enum_member_catalog_id(&program, "Status", "archived")
    );
    assert_eq!(
        run_entry(
            &store,
            checked_entry!(&program, "test::matches_archived", Value::Int(7))
        )
        .expect("compare")
        .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn an_entry_enum_argument_uses_checked_catalog_identity() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n\n\
         pub fn give(): Status\n    return Status::active\n\n\
         pub fn save(state: Status)\n    ^orders(1).state = state\n\n\
         pub fn read(): bool\n    return (^orders(1).state ?? Status::archived) == Status::active\n",
    );
    let store = TreeStore::memory();
    let state = run_entry(&store, checked_entry!(&program, "test::give"))
        .expect("give")
        .value
        .expect("enum value");

    run_entry(&store, checked_entry!(&program, "test::save", state)).expect("save enum argument");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::read"))
            .expect("read")
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn an_enum_index_uses_catalog_member_keys() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         resource Order\n    required state: Status\n\
         store ^orders(id: int): Order\n    index byState(state, id)\n\n\
         pub fn seed()\n    ^orders(1).state = Status::archived\n    ^orders(2).state = Status::active\n    ^orders(3).state = Status::archived\n\n\
         pub fn countArchived(): int\n    var count = 0\n    for id in ^orders.byState(Status::archived)\n        count = count + 1\n    return count\n\n\
         pub fn countActive(): int\n    var count = 0\n    for id in ^orders.byState(Status::active)\n        count = count + 1\n    return count\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed enum-index fixture");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countArchived"))
            .expect("count archived")
            .value,
        Some(Value::Int(2))
    );
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::countActive"))
            .expect("count active")
            .value,
        Some(Value::Int(1))
    );
}

#[test]
fn a_singleton_keyed_enum_leaf_can_be_matched_after_read() {
    let program = checked_program(
        "enum Kind\n    number\n    plus\n\n\
         resource Session\n    required cursor: int\n    kinds(pos: int): Kind\n\
         store ^session: Session\n\n\
         pub fn readBack(): int\n    \
         ^session.cursor = 1\n    \
         ^session.kinds(1) = Kind::plus\n    \
         match ^session.kinds(1) ?? Kind::number\n        number\n            return 0\n        plus\n            return 1\n",
    );
    let store = TreeStore::memory();
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::readBack"))
            .expect("keyed enum leaf match runs")
            .value,
        Some(Value::Int(1))
    );
}

/// Nominal `==` on enum values: comparing the same member is true, comparing two
/// different members of the same enum is false.
#[test]
fn enum_equality_is_true_for_the_same_member_and_false_otherwise() {
    let program = checked_program(
        "enum Color\n    red\n    green\n    blue\n\n\
         pub fn same(): bool\n    return Color::green == Color::green\n\n\
         pub fn different(): bool\n    return Color::green == Color::blue\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::same")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::different")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `match` dispatches to the arm for the scrutinee's member. Each arm returns a
/// distinct marker, so the returned value names the chosen arm.
#[test]
fn match_dispatches_to_the_arm_for_the_scrutinees_member() {
    let program = checked_program(
        "enum Status\n    active\n    archived\n    banned\n\n\
         pub fn label(s: Status): int\n    \
         match s\n        active\n            return 10\n        \
         archived\n            return 20\n        banned\n            return 30\n\n\
         pub fn labelActive(): int\n    return label(Status::active)\n\n\
         pub fn labelArchived(): int\n    return label(Status::archived)\n\n\
         pub fn labelBanned(): int\n    return label(Status::banned)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelActive")).unwrap(),
        Some(Value::Int(10))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelArchived")).unwrap(),
        Some(Value::Int(20))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelBanned")).unwrap(),
        Some(Value::Int(30))
    );
}

/// `match` dispatches by the scrutinee's resolved enum, not by the arm member
/// set. Two enums share member names `x` and `y` but in opposite declaration
/// order, so dispatching by the arm set alone would pick `A` and invert the
/// result for `B`.
#[test]
fn match_dispatches_by_the_scrutinees_enum_not_the_arm_set() {
    let program = checked_program(
        "enum A\n    x\n    y\n\n\
         enum B\n    y\n    x\n\n\
         pub fn label(s: B): int\n    \
         match s\n        x\n            return 1\n        y\n            return 2\n\n\
         pub fn labelX(): int\n    return label(B::x)\n\n\
         pub fn labelY(): int\n    return label(B::y)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelX")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelY")).unwrap(),
        Some(Value::Int(2))
    );
}

/// Nested enum member paths resolve to distinct concrete members.
#[test]
fn a_nested_member_path_resolves_to_the_right_member() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn bengalMatches(): bool\n    return Cat::bengal == Cat::tiger::bengal\n\n\
         pub fn bengalIsHousecat(): bool\n    return Cat::bengal == Cat::housecat\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalMatches")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsHousecat")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `pet is Cat::tiger` is true for any value at or under `tiger` (a `bengal`),
/// false for one outside it (a `housecat`).
#[test]
fn is_tests_subtree_membership() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn bengalIsTiger(): bool\n    return Cat::bengal is Cat::tiger\n\n\
         pub fn housecatIsTiger(): bool\n    return Cat::housecat is Cat::tiger\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsTiger")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::housecatIsTiger")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// `is` against a concrete leaf is exact equality: a `bengal` value is
/// `Cat::bengal` but not `Cat::siberian`.
#[test]
fn is_against_a_leaf_is_exact() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn bengalIsBengal(): bool\n    return Cat::bengal is Cat::bengal\n\n\
         pub fn siberianIsBengal(): bool\n    return Cat::siberian is Cat::bengal\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::bengalIsBengal")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::siberianIsBengal")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// A `match` runs the category arm for any descendant: a `bengal` or `siberian`
/// value both take the `tiger` arm, a `housecat` takes its own.
#[test]
fn match_runs_the_category_arm_for_any_descendant() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        siberian\n    housecat\n\n\
         pub fn label(pet: Cat): int\n    \
         match pet\n        tiger\n            return 1\n        \
         housecat\n            return 2\n\n\
         pub fn labelBengal(): int\n    return label(Cat::bengal)\n\n\
         pub fn labelSiberian(): int\n    return label(Cat::siberian)\n\n\
         pub fn labelHousecat(): int\n    return label(Cat::housecat)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelBengal")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelSiberian")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelHousecat")).unwrap(),
        Some(Value::Int(2))
    );
}

/// Two `paw`s under different parents are distinct members: the full member paths
/// `Cat::tiger::paw` and `Cat::lion::paw` are not aliases.
#[test]
fn a_duplicated_member_resolves_by_its_full_path_to_a_distinct_member() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         pub fn sameTigerPaw(): bool\n    return Cat::tiger::paw == Cat::tiger::paw\n\n\
         pub fn differentPaws(): bool\n    return Cat::tiger::paw == Cat::lion::paw\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::sameTigerPaw")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::differentPaws")).unwrap(),
        Some(Value::Bool(false))
    );
}

/// A `match` with qualified arms over duplicated leaves dispatches each value to
/// the correct arm by walking the arm path against the scrutinee enum — the two
/// `paw`s take different arms even though they share a name.
#[test]
fn match_with_qualified_arms_dispatches_each_duplicated_paw_to_its_own_arm() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         pub fn label(pet: Cat): int\n    \
         match pet\n        \
         tiger::bengal\n            return 1\n        \
         tiger::paw\n            return 2\n        \
         lion::paw\n            return 3\n        \
         lion::mane\n            return 4\n\n\
         pub fn labelTigerBengal(): int\n    return label(Cat::tiger::bengal)\n\n\
         pub fn labelTigerPaw(): int\n    return label(Cat::tiger::paw)\n\n\
         pub fn labelLionPaw(): int\n    return label(Cat::lion::paw)\n\n\
         pub fn labelLionMane(): int\n    return label(Cat::lion::mane)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelTigerPaw")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelLionPaw")).unwrap(),
        Some(Value::Int(3))
    );
    // The other leaves still dispatch to their arms.
    assert_eq!(
        run(checked_entry!(&program, "test::labelTigerBengal")).unwrap(),
        Some(Value::Int(1))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::labelLionMane")).unwrap(),
        Some(Value::Int(4))
    );
}

/// `is` with a full member path is exact over the right leaf, and a category right
/// operand is the subtree test — the same `is_descendant` walk, now reachable for
/// a duplicated leaf via its qualifying path.
#[test]
fn is_with_a_full_path_to_a_duplicated_leaf_is_exact() {
    let program = checked_program(
        "enum Cat\n    category tiger\n        bengal\n        paw\n\
         \x20   category lion\n        paw\n        mane\n\n\
         pub fn tigerPawIsTigerPaw(): bool\n    return Cat::tiger::paw is Cat::tiger::paw\n\n\
         pub fn lionPawIsTigerPaw(): bool\n    return Cat::lion::paw is Cat::tiger::paw\n\n\
         pub fn tigerPawIsTiger(): bool\n    return Cat::tiger::paw is Cat::tiger\n\n\
         pub fn lionPawIsTiger(): bool\n    return Cat::lion::paw is Cat::tiger\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tigerPawIsTigerPaw")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lionPawIsTigerPaw")).unwrap(),
        Some(Value::Bool(false))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::tigerPawIsTiger")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::lionPawIsTiger")).unwrap(),
        Some(Value::Bool(false))
    );
}
