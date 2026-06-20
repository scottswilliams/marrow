//! Local keyed-collection parameters: a `var scores(player: string): int` value
//! passes to a function exactly as a `sequence[T]` does — by value, with the same
//! call-argument typing and the same no-mutation-through-the-parameter rule.

use crate::support;
use support::*;

use marrow_run::Value;

#[test]
fn function_iterates_a_local_keyed_map_parameter() {
    // A scratch keyed map built in `main` passes into `total`, which iterates and
    // reads it to compute a value — the FINDINGS use-case, with no saved store.
    let program = checked_program(
        "pub fn total(scores(player: string): int): int\n\
         \x20   var sum = 0\n\
         \x20   for player in scores\n\
         \x20       sum = sum + (scores(player) ?? 0)\n\
         \x20   return sum\n\
         pub fn main(): int\n\
         \x20   var scores(player: string): int\n\
         \x20   scores(\"a\") = 3\n\
         \x20   scores(\"b\") = 4\n\
         \x20   return total(scores)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")).unwrap(),
        Some(Value::Int(7))
    );
}

#[test]
fn function_counts_a_local_keyed_map_parameter() {
    let program = checked_program(
        "pub fn size(scores(player: string): int): int\n\
         \x20   return count(scores)\n\
         pub fn main(): int\n\
         \x20   var scores(player: string): int\n\
         \x20   scores(\"a\") = 1\n\
         \x20   scores(\"b\") = 2\n\
         \x20   scores(\"c\") = 3\n\
         \x20   return size(scores)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")).unwrap(),
        Some(Value::Int(3))
    );
}

#[test]
fn a_keyed_map_parameter_passes_by_value_and_leaves_the_caller_unchanged() {
    // The callee receives an owned copy of the caller's map and reads it without
    // changing the caller value: after the call the caller's own count is intact.
    let program = checked_program(
        "pub fn size(scores(player: string): int): int\n\
         \x20   return count(scores)\n\
         pub fn main(): int\n\
         \x20   var scores(player: string): int\n\
         \x20   scores(\"a\") = 3\n\
         \x20   scores(\"b\") = 4\n\
         \x20   var inside = size(scores)\n\
         \x20   return inside * 1000 + count(scores)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::main")).unwrap(),
        Some(Value::Int(2002))
    );
}

#[test]
fn writing_a_keyed_map_parameter_is_rejected_as_read_only() {
    // A parameter is read-only by value, so writing the caller's map through the
    // parameter is a clean check error — the caller cannot be mutated through the
    // parameter, the same no-mutation rule a `sequence` parameter follows.
    checker_rejects(
        "pub fn bump(scores(player: string): int): int\n\
         \x20   scores(\"a\") = 100\n\
         \x20   return count(scores)\n\
         pub fn main(): int\n\
         \x20   var scores(player: string): int\n\
         \x20   scores(\"a\") = 3\n\
         \x20   return bump(scores)\n",
        "check.invalid_assign_target",
    );
}

#[test]
fn passing_a_mismatched_local_value_to_a_keyed_parameter_is_a_check_error() {
    // A plain `int` argument cannot satisfy a keyed-map parameter: the call is a
    // clean `check.call_argument`, the same rule a wrong sequence element triggers.
    checker_rejects(
        "pub fn total(scores(player: string): int): int\n\
         \x20   return count(scores)\n\
         pub fn main(): int\n\
         \x20   var n = 5\n\
         \x20   return total(n)\n",
        "check.call_argument",
    );
}

#[test]
fn an_unknown_key_type_on_a_keyed_parameter_is_an_unknown_type_error() {
    // A keyed parameter's key annotation is validated like every other type
    // annotation, so a nonexistent key type is a clean `check.unknown_type`.
    checker_rejects(
        "pub fn total(scores(player: Bogus): int): int\n\
         \x20   return count(scores)\n",
        "check.unknown_type",
    );
}

#[test]
fn passing_a_wrong_key_type_to_a_keyed_parameter_is_a_check_error() {
    // The argument's key column type must match the parameter's: an `int`-keyed map
    // does not satisfy a `string`-keyed parameter.
    checker_rejects(
        "pub fn total(scores(player: string): int): int\n\
         \x20   return count(scores)\n\
         pub fn main(): int\n\
         \x20   var byNum(n: int): int\n\
         \x20   byNum(1) = 9\n\
         \x20   return total(byNum)\n",
        "check.call_argument",
    );
}
