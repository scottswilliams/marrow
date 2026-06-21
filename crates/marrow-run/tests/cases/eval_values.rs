//! Scalar value evaluation: arithmetic, decimal and bytes literals, base64,
//! local sequences and trees, and the std math decimal helpers.

use crate::support;
use support::*;

use marrow_run::{RUN_DECIMAL_OVERFLOW, RUN_DIVIDE_BY_ZERO, RUN_TYPE, RUN_UNSUPPORTED, Value};
use marrow_store::tree::TreeStore;

#[test]
#[should_panic(expected = "runtime tests require a clean checked program")]
fn runtime_test_helper_rejects_checker_error_programs() {
    checked_program_with_imports(
        "pub fn stamp(s: string): string\n    return clock::formatDate(clock::parseDate(s))\n",
        &[],
    );
}

#[test]
fn evaluates_arithmetic_over_parameters() {
    assert_eq!(
        eval_source(
            "pub fn add(a: int, b: int): int\n    return a + b\n",
            "add",
            vec![Value::Int(2), Value::Int(40)]
        ),
        Ok(Some(Value::Int(42)))
    );
}

#[test]
fn respects_arithmetic_precedence() {
    // 2 + 3 * 4 == 14, not 20.
    assert_eq!(
        eval_source("pub fn f(): int\n    return 2 + 3 * 4\n", "f", Vec::new()),
        Ok(Some(Value::Int(14)))
    );
}

#[test]
fn evaluates_decimal_literals_and_arithmetic() {
    // Decimal `+`, `*`, and `-` over decimal operands, rendered to text.
    let program = checked_program(
        "pub fn f(): string\n    return $\"{1.5 + 2.5} {1.5 * 2.0} {5.5 - 0.5}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("4 3 5".into()))
    );
}

#[test]
fn negates_a_decimal() {
    // Unary `-` on a decimal, and a subtraction that produces a negative decimal.
    let program = checked_program("pub fn f(): string\n    return $\"{-1.5} {0.0 - 2.5}\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("-1.5 -2.5".into()))
    );
}

#[test]
fn division_yields_a_decimal() {
    // `/` always yields a decimal, even for integer operands (1/2 = 0.5).
    let program =
        checked_program("pub fn f(): string\n    return $\"{1 / 2} {7 / 2} {1.0 / 4.0}\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("0.5 3.5 0.25".into()))
    );
}

#[test]
fn decimal_division_rounds_half_even() {
    // 1/3 rounds half-even to 34 significant digits.
    let program = checked_program("pub fn f(): string\n    return $\"{1 / 3}\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str(format!("0.{}", "3".repeat(34))))
    );
}

#[test]
fn decimal_multiplication_must_fit_exactly() {
    let program = checked_program(
        "pub fn f(): decimal\n    return 0.123456789012345678 * 0.123456789012345678\n",
    );
    assert_run_error(
        run(checked_entry!(&program, "test::f")),
        RUN_DECIMAL_OVERFLOW,
    );
}

#[test]
fn decimal_division_by_zero_is_a_runtime_error() {
    let program = checked_program("pub fn f(): decimal\n    return 1.0 / 0.0\n");
    assert_run_error(run(checked_entry!(&program, "test::f")), RUN_DIVIDE_BY_ZERO);
}

#[test]
fn compares_decimal_values() {
    // Ordering and equality compare by value (1.50 equals 1.5).
    let program = checked_program(
        "pub fn f(): string\n    return $\"{1.5 < 2.0} {1.50 == 1.5} {2.5 > 3.0}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("true true false".into()))
    );
}

#[test]
fn decimal_round_trips_through_saved_data() {
    // A decimal field saves and loads unchanged.
    let program = checked_program(
        "resource Account\n\
         \x20   balance: decimal\nstore ^accts(id: int): Account\n\
         \n\
         pub fn seed()\n\
         \x20   ^accts(1).balance = 9.99\n\
         \n\
         pub fn balance(): string\n\
         \x20   return $\"{^accts(1).balance ?? 0.0}\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::balance"))
            .unwrap()
            .value,
        Some(Value::Str("9.99".into()))
    );
}

#[test]
fn evaluates_bytes_literals_and_equality() {
    let program = checked_program(
        "pub fn same(): bool\n    return b\"abc\" == b\"abc\"\n\n\
         pub fn different(): bool\n    return b\"abc\" == b\"abd\"\n",
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

#[test]
fn bytes_escapes_are_decoded() {
    let program = checked_program(
        "pub fn f(): bytes\n    return b\"slash \\\\ quote \\\" line\\n carriage\\r tab\\t hex \\x00\\x7f\\xff café\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Bytes(
            b"slash \\ quote \" line\n carriage\r tab\t hex \x00\x7f\xff caf\xc3\xa9".to_vec()
        ))
    );
}

#[test]
fn malformed_bytes_escapes_are_rejected_at_check_time() {
    // Bytes escapes are static, so the checker rejects a malformed one before a
    // run rather than letting it fault at runtime.
    for source in [
        "pub fn f(): bytes\n    return b\"\\q\"\n",
        "pub fn f(): bytes\n    return b\"\\x0g\"\n",
        "pub fn f(): bytes\n    return b\"\\x0\"\n",
    ] {
        checker_rejects(source, "check.bytes_escape");
    }
}

#[test]
fn compares_bytes_by_byte_order() {
    let program = checked_program(
        "pub fn f(): bool\n    return b\"a\" < b\"b\"\n\n\
         pub fn g(): bool\n    return b\"ab\" > b\"a\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::g")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn bytes_round_trip_through_saved_data() {
    let program = checked_program(
        "resource Blob\n\
         \x20   data: bytes\nstore ^blobs(id: int): Blob\n\
         \n\
         pub fn seed()\n\
         \x20   ^blobs(1).data = b\"xy\"\n\
         \n\
         pub fn matches(): bool\n\
         \x20   return (^blobs(1).data ?? b\"\") == b\"xy\"\n",
    );
    let store = TreeStore::memory();
    run_entry(&store, checked_entry!(&program, "test::seed")).expect("seed runs");
    assert_eq!(
        run_entry(&store, checked_entry!(&program, "test::matches"))
            .unwrap()
            .value,
        Some(Value::Bool(true))
    );
}

#[test]
fn converts_string_to_bytes_and_measures_length() {
    let program = checked_program(
        "pub fn short(): int\n    return std::bytes::length(bytes(\"hi\"))\n\n\
         pub fn utf8(): int\n    return std::bytes::length(bytes(\"café\"))\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::short")).unwrap(),
        Some(Value::Int(2))
    );
    // `café` is 4 characters but 5 UTF-8 bytes; std::bytes::length counts bytes.
    assert_eq!(
        run(checked_entry!(&program, "test::utf8")).unwrap(),
        Some(Value::Int(5))
    );
}

#[test]
fn bytes_conversion_equals_a_bytes_literal() {
    let program = checked_program("pub fn f(): bool\n    return bytes(\"xy\") == b\"xy\"\n");
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn base64_encodes_with_padding() {
    let program = checked_program(
        "pub fn a(): string\n    return std::bytes::base64Encode(b\"hello\")\n\n\
         pub fn b(): string\n    return std::bytes::base64Encode(b\"a\")\n\n\
         pub fn c(): string\n    return std::bytes::base64Encode(b\"ab\")\n\n\
         pub fn d(): string\n    return std::bytes::base64Encode(b\"abc\")\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::a")).unwrap(),
        Some(Value::Str("aGVsbG8=".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::b")).unwrap(),
        Some(Value::Str("YQ==".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::c")).unwrap(),
        Some(Value::Str("YWI=".into()))
    );
    // An exact 3-byte group needs no padding.
    assert_eq!(
        run(checked_entry!(&program, "test::d")).unwrap(),
        Some(Value::Str("YWJj".into()))
    );
}

#[test]
fn base64_decodes_and_round_trips() {
    let program = checked_program(
        "pub fn known(): bool\n    return std::bytes::base64Decode(\"aGVsbG8=\") == b\"hello\"\n\n\
         pub fn round(): bool\n    return std::bytes::base64Decode(std::bytes::base64Encode(b\"hi there\")) == b\"hi there\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::known")).unwrap(),
        Some(Value::Bool(true))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::round")).unwrap(),
        Some(Value::Bool(true))
    );
}

#[test]
fn base64_decode_rejects_invalid_text() {
    // Invalid characters, and `=` padding outside the final group.
    let program = checked_program(
        "pub fn bad_chars(): bytes\n    return std::bytes::base64Decode(\"!!!!\")\n\n\
         pub fn early_pad(): bytes\n    return std::bytes::base64Decode(\"AAA=AAAA\")\n",
    );
    assert_run_error(run(checked_entry!(&program, "test::bad_chars")), RUN_TYPE);
    assert_run_error(run(checked_entry!(&program, "test::early_pad")), RUN_TYPE);
}

#[test]
fn splits_a_string_and_iterates_the_sequence() {
    // `std::text::split` yields a sequence; `values(...)` binds its element values
    // in order, while the bare sequence binds positions.
    let program = checked_program(
        "pub fn f(): string\n\
         \x20   var result = \"\"\n\
         \x20   for word in values(std::text::split(\"a,b,c\", \",\"))\n\
         \x20       result = result + word\n\
         \x20   return result\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::f")).unwrap(),
        Some(Value::Str("abc".into()))
    );
}

#[test]
fn iterates_a_sequence_binding_its_positions() {
    // A bare local sequence binds its 1-based positions, so summing the loop
    // variable over a four-element sequence yields 1+2+3+4.
    let program = checked_program(
        "pub fn split_positions(): int\n\
         \x20   var total = 0\n\
         \x20   for pos in std::text::split(\"a,b,c,d\", \",\")\n\
         \x20       total = total + pos\n\
         \x20   return total\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::split_positions")).unwrap(),
        Some(Value::Int(10))
    );
}

#[test]
fn two_name_loop_over_a_range_is_unsupported() {
    let program = checked_program(
        "pub fn f()\n\
         \x20   for start, stop in 1..3\n\
         \x20       print($\"{start}{stop}\")\n",
    );
    assert_run_error(run(checked_entry!(&program, "test::f")), RUN_UNSUPPORTED);
}

#[test]
fn append_and_count_over_a_local_sequence_are_typed_int() {
    // `append` returns the appended position and `count` returns the element count,
    // both `int`; the bare sequence loop then sums its 1-based positions.
    let program = checked_program(
        "pub fn grow(): int\n\
         \x20   var order: sequence[int]\n\
         \x20   const first: int = append(order, 10)\n\
         \x20   const second: int = append(order, 20)\n\
         \x20   var total = first * 100 + second * 10 + count(order)\n\
         \x20   for pos in order\n\
         \x20       total = total + pos\n\
         \x20   return total\n",
    );
    // first=1, second=2, count=2: 100 + 20 + 2 = 122; positions 1+2 add 3 → 125.
    assert_eq!(
        run(checked_entry!(&program, "test::grow")).unwrap(),
        Some(Value::Int(125))
    );
}

#[test]
fn reads_and_writes_a_local_sequence_by_position() {
    // A positional read is maybe-present, so each read resolves with `??`; the
    // write target on the left of `=` is a place, not a read.
    let program = checked_program(
        "pub fn seq_index(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   xs(1) = 10\n\
         \x20   xs(1) = (xs(1) ?? 0) + 5\n\
         \x20   return xs(1) ?? -1\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq_index")).unwrap(),
        Some(Value::Int(15))
    );
}

#[test]
fn guards_resolve_present_and_absent_local_collection_reads() {
    // A positional sequence read and a keyed-tree read are maybe-present: `??`
    // yields the value when present and the default when absent, and `if const`
    // takes its else branch on an absent key. The runtime resolves each at the read
    // site by catching the absent fault, never surfacing `run.absent_element`.
    let program = checked_program(
        "pub fn probe(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   var counts(k: string): int\n\
         \x20   counts(\"a\") = 5\n\
         \x20   var total = (xs(1) ?? -1) + (xs(9) ?? -1)\n\
         \x20   total = total + (counts(\"a\") ?? -1)\n\
         \x20   if const missing = counts(\"absent\")\n\
         \x20       total = total + missing\n\
         \x20   else\n\
         \x20       total = total + 1000\n\
         \x20   return total\n",
    );
    // present 10, absent -1, present 5, missing key adds 1000: 10 - 1 + 5 + 1000.
    assert_eq!(
        run(checked_entry!(&program, "test::probe")).unwrap(),
        Some(Value::Int(1014))
    );
}

#[test]
fn guards_resolve_a_non_positive_sequence_position() {
    // A position outside the 1-based domain has no node, so `xs(0)`/`xs(-1)` is a
    // hole the same as any out-of-range read. The guard catches the absent fault
    // and yields the fallback rather than aborting with an uncatchable type fault,
    // so the spec's "resolved at the read site" holds for every int position.
    let program = checked_program(
        "pub fn probe(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   var total = (xs(0) ?? -1) + (xs(-1) ?? -2)\n\
         \x20   if const v = xs(0)\n\
         \x20       total = total + v\n\
         \x20   else\n\
         \x20       total = total + 100\n\
         \x20   if exists(xs(-5))\n\
         \x20       total = total + 1000\n\
         \x20   else\n\
         \x20       total = total + 10\n\
         \x20   return total\n",
    );
    // xs(0) -> -1, xs(-1) -> -2, if const else 100, if exists else 10: -1 -2 +100 +10.
    assert_eq!(
        run(checked_entry!(&program, "test::probe")).unwrap(),
        Some(Value::Int(107))
    );
}

#[test]
fn a_dynamic_non_positive_local_sequence_write_faults_persists_nothing_and_names_the_position() {
    // A local sequence is identical to a saved sequence: a position below 1 addresses
    // no node, so a dynamic write to one raises the catchable absent fault, mutates
    // nothing, and names the position rather than claiming the whole collection is
    // absent. After catching it, the binding still holds only its one stored element.
    let program = checked_program(
        "pub fn probe(pos: int): string\n\
         \x20   var xs: sequence[string]\n\
         \x20   append(xs, \"a\")\n\
         \x20   var caught: string = \"none\"\n\
         \x20   try\n\
         \x20       xs(pos) = \"bad\"\n\
         \x20   catch err: Error\n\
         \x20       caught = err.message\n\
         \x20   return $\"{caught}|{count(xs)}|{xs(1) ?? \"gone\"}\"\n",
    );
    for pos in [0, -5] {
        assert_eq!(
            run(checked_entry!(&program, "test::probe", Value::Int(pos))).unwrap(),
            Some(Value::Str(
                "a sequence position below 1 is absent|1|a".into()
            )),
            "pos {pos}: the dynamic write must fault, persist nothing, and name the position",
        );
    }
}

#[test]
fn a_dynamic_non_positive_local_int_keyed_tree_write_faults_and_persists_nothing() {
    // A local single int-keyed tree is a 1-based sequence too, so a dynamic write to a
    // position below 1 faults and persists nothing — it must never accept, store, or
    // count a zero or negative key. A positive key written alongside it survives.
    let program = checked_program(
        "pub fn probe(pos: int): string\n\
         \x20   var t(k: int): string\n\
         \x20   t(1) = \"one\"\n\
         \x20   var caught: string = \"none\"\n\
         \x20   try\n\
         \x20       t(pos) = \"bad\"\n\
         \x20   catch err: Error\n\
         \x20       caught = err.message\n\
         \x20   return $\"{caught}|{count(t)}|{t(1) ?? \"gone\"}|{t(pos) ?? \"absent\"}\"\n",
    );
    for pos in [0, -3] {
        assert_eq!(
            run(checked_entry!(&program, "test::probe", Value::Int(pos))).unwrap(),
            Some(Value::Str(
                "a sequence position below 1 is absent|1|one|absent".into()
            )),
            "pos {pos}: the keyed-tree write must fault, persist nothing, and stay readable as absent",
        );
    }
}

#[test]
fn a_non_positive_string_keyed_tree_write_still_persists() {
    // A string-keyed local tree is not a sequence, so a key that happens to be "0"
    // carries meaning in its own right and is a legitimate, persisted write. The
    // 1-based guard fires only on a single int key column.
    let program = checked_program(
        "pub fn probe(): string\n\
         \x20   var scores(player: string): int\n\
         \x20   scores(\"0\") = 5\n\
         \x20   return $\"{count(scores)}|{scores(\"0\") ?? -1}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::probe")).unwrap(),
        Some(Value::Str("1|5".into()))
    );
}

#[test]
fn local_sequence_writes_a_sparse_position_past_the_end() {
    // A local sequence is a 1-based integer-keyed tree, so writing past the dense
    // range leaves a hole rather than faulting. `xs(5)` after one element fills
    // position 5; positions 2..4 stay holes that read absent, exactly as a saved
    // sequence does.
    let program = checked_program(
        "pub fn sparse(): string\n\
         \x20   var xs: sequence[string]\n\
         \x20   append(xs, \"a\")\n\
         \x20   xs(5) = \"sparse\"\n\
         \x20   const at5: string = xs(5) ?? \"none\"\n\
         \x20   const at3: string = xs(3) ?? \"hole\"\n\
         \x20   const n: int = count(xs)\n\
         \x20   return $\"{at5};{at3};{n}\"\n",
    );
    // position 5 reads "sparse", the hole at 3 reads the fallback, and count is the
    // two stored entries (1 and 5), not the highest key.
    assert_eq!(
        run(checked_entry!(&program, "test::sparse")).unwrap(),
        Some(Value::Str("sparse;hole;2".into()))
    );
}

#[test]
fn local_sequence_iteration_visits_only_stored_positions() {
    // After a sparse write the positions are 1 and 5; the bare loop binds those
    // stored positions in key order and skips the holes, `values` binds their
    // elements, and `reversed` walks them descending — the stored-only, gap-skipping
    // walk a saved sequence guarantees.
    let program = checked_program(
        "pub fn walk(): string\n\
         \x20   var xs: sequence[string]\n\
         \x20   append(xs, \"a\")\n\
         \x20   xs(5) = \"e\"\n\
         \x20   var out: string = \"\"\n\
         \x20   for pos in xs\n\
         \x20       out = $\"{out}{pos};\"\n\
         \x20   for value in values(xs)\n\
         \x20       out = $\"{out}v{value};\"\n\
         \x20   for pos in reversed(xs)\n\
         \x20       out = $\"{out}r{pos};\"\n\
         \x20   for pos, value in entries(xs)\n\
         \x20       out = $\"{out}e{pos}={value};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::walk")).unwrap(),
        Some(Value::Str("1;5;va;ve;r5;r1;e1=a;e5=e;".into()))
    );
}

#[test]
fn local_sequence_append_skips_holes_choosing_after_the_highest_position() {
    // Append chooses one past the highest populated position, never filling a hole,
    // matching the saved-sequence append contract. After a hole at 5, append lands
    // at 6.
    let program = checked_program(
        "pub fn grow(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 1)\n\
         \x20   xs(5) = 50\n\
         \x20   const at: int = append(xs, 6)\n\
         \x20   return at\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::grow")).unwrap(),
        Some(Value::Int(6))
    );
}

#[test]
fn local_sequence_delete_leaves_a_hole() {
    // Deleting a position removes that entry and leaves a hole, exactly as deleting a
    // saved sequence position does. Append after a delete does not reuse the hole,
    // and iteration skips the deleted position.
    let program = checked_program(
        "pub fn drop(): string\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   append(xs, 20)\n\
         \x20   append(xs, 30)\n\
         \x20   delete xs(2)\n\
         \x20   const gone: int = xs(2) ?? -1\n\
         \x20   const n: int = count(xs)\n\
         \x20   const at: int = append(xs, 40)\n\
         \x20   var positions: string = \"\"\n\
         \x20   for pos in xs\n\
         \x20       positions = $\"{positions}{pos};\"\n\
         \x20   return $\"{gone};{n};{at};{positions}\"\n",
    );
    // position 2 reads absent, count is the two remaining stored entries, append lands
    // at 4 (past the highest 3, not reusing the hole at 2), and iteration visits 1,3,4.
    assert_eq!(
        run(checked_entry!(&program, "test::drop")).unwrap(),
        Some(Value::Str("-1;2;4;1;3;4;".into()))
    );
}

#[test]
fn local_sequence_delete_of_an_absent_position_is_a_no_op() {
    // Deleting a hole or an out-of-range position removes nothing and is tolerant,
    // the same no-op as deleting any absent saved position.
    let program = checked_program(
        "pub fn drop(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 10)\n\
         \x20   delete xs(9)\n\
         \x20   delete xs(0)\n\
         \x20   return count(xs)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::drop")).unwrap(),
        Some(Value::Int(1))
    );
}

#[test]
fn single_name_loop_over_a_local_sequence_binds_one_based_positions() {
    // A local sequence is a 1-based integer-keyed tree, so a single loop variable
    // binds its position, mirroring a saved sequence and a local keyed tree.
    let program = checked_program(
        "pub fn seq(): string\n\
         \x20   var xs: sequence[int]\n\
         \x20   xs(1) = 10\n\
         \x20   xs(2) = 20\n\
         \x20   xs(3) = 30\n\
         \x20   var out: string = \"\"\n\
         \x20   for pos in xs\n\
         \x20       const typed: int = pos\n\
         \x20       out = $\"{out}{pos};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq")).unwrap(),
        Some(Value::Str("1;2;3;".into()))
    );
}

#[test]
fn two_name_loop_over_a_local_sequence_binds_position_and_value() {
    let program = checked_program(
        "pub fn seq(): string\n\
         \x20   var xs: sequence[int]\n\
         \x20   xs(1) = 10\n\
         \x20   xs(2) = 20\n\
         \x20   xs(3) = 30\n\
         \x20   var out: string = \"\"\n\
         \x20   for pos, value in xs\n\
         \x20       const typedPos: int = pos\n\
         \x20       const typedValue: int = value\n\
         \x20       out = $\"{out}{pos}={value};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq")).unwrap(),
        Some(Value::Str("1=10;2=20;3=30;".into()))
    );
}

#[test]
fn local_sequence_value_and_entry_views_stay_value_based() {
    let program = checked_program(
        "pub fn seq(): string\n\
         \x20   var xs: sequence[int]\n\
         \x20   xs(1) = 10\n\
         \x20   xs(2) = 20\n\
         \x20   xs(3) = 30\n\
         \x20   var out: string = \"\"\n\
         \x20   for value in values(xs)\n\
         \x20       const typed: int = value\n\
         \x20       out = $\"{out}v{value};\"\n\
         \x20   for pos in reversed(xs)\n\
         \x20       const typedPos: int = pos\n\
         \x20       out = $\"{out}r{pos};\"\n\
         \x20   for value in reversed(values(xs))\n\
         \x20       const typedValue: int = value\n\
         \x20       out = $\"{out}rv{value};\"\n\
         \x20   for pos, value in entries(xs)\n\
         \x20       out = $\"{out}e{pos}={value};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq")).unwrap(),
        Some(Value::Str(
            "v10;v20;v30;r3;r2;r1;rv30;rv20;rv10;e1=10;e2=20;e3=30;".into()
        ))
    );
}

#[test]
fn count_over_a_local_sequence_types_int() {
    // `count` of a local collection returns `int`, usable in a typed binding and
    // arithmetic, exactly as `count` of a saved path does.
    let program = checked_program(
        "pub fn seq(): int\n\
         \x20   var xs: sequence[int]\n\
         \x20   xs(1) = 10\n\
         \x20   xs(2) = 20\n\
         \x20   xs(3) = 30\n\
         \x20   const n: int = count(xs)\n\
         \x20   return n + 1\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq")).unwrap(),
        Some(Value::Int(4))
    );
}

#[test]
fn count_over_a_local_keyed_tree_types_int() {
    let program = checked_program(
        "pub fn keyed(): int\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   const n: int = count(scores)\n\
         \x20   return n\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Int(2))
    );
}

#[test]
fn two_name_loop_over_a_local_keyed_tree_binds_key_and_value() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId, score in scores\n\
         \x20       out = $\"{out}{playerId}={score};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p1=10;p2=20;".into()))
    );
}

#[test]
fn two_name_reversed_loop_over_a_local_keyed_tree_binds_descending_pairs() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId, score in reversed(scores)\n\
         \x20       const typedPlayer: string = playerId\n\
         \x20       const typedScore: int = score\n\
         \x20       out = $\"{out}{playerId}={score};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p2=20;p1=10;".into()))
    );
}

#[test]
fn single_name_loop_over_a_local_keyed_tree_binds_keys() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in scores\n\
         \x20       const typed: string = playerId\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p1;p2;".into()))
    );
}

#[test]
fn keys_over_reversed_local_keyed_tree_yields_descending_keys() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in keys(reversed(scores))\n\
         \x20       const typed: string = playerId\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p2;p1;".into()))
    );
}

#[test]
fn reversed_keys_over_local_keyed_tree_yields_descending_keys() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in reversed(keys(scores))\n\
         \x20       const typed: string = playerId\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p2;p1;".into()))
    );
}

#[test]
fn reversed_keys_over_reversed_local_keyed_tree_yields_ascending_keys() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in reversed(keys(reversed(scores)))\n\
         \x20       const typed: string = playerId\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p1;p2;".into()))
    );
}

#[test]
fn materialized_reversed_keys_over_reversed_local_keyed_tree_yields_ascending_keys() {
    // `reversed(keys(reversed(scores)))` materializes a sequence whose values are
    // the keys; iterating that sequence binds positions, so the captured keys are
    // its `values(...)` view.
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   const players = reversed(keys(reversed(scores)))\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in values(players)\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p1;p2;".into()))
    );
}

#[test]
fn reversed_loop_over_a_local_keyed_tree_binds_descending_keys() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in reversed(scores)\n\
         \x20       const typed: string = playerId\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p2;p1;".into()))
    );
}

#[test]
fn reversed_local_keyed_tree_materializes_a_key_sequence() {
    // `reversed(scores)` materializes a sequence of the descending keys; iterating
    // it binds positions, so the captured keys are its `values(...)` view.
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   const players = reversed(scores)\n\
         \x20   var out: string = \"\"\n\
         \x20   for playerId in values(players)\n\
         \x20       const typed: string = playerId\n\
         \x20       out = $\"{out}{playerId};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str("p2;p1;".into()))
    );
}

#[test]
fn local_keyed_tree_value_and_entry_views_stay_value_based() {
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(playerId: string): int\n\
         \x20   scores(\"p2\") = 20\n\
         \x20   scores(\"p1\") = 10\n\
         \x20   var out: string = \"\"\n\
         \x20   for score in values(scores)\n\
         \x20       const typedScore: int = score\n\
         \x20       out = $\"{out}v{score};\"\n\
         \x20   for score in reversed(values(scores))\n\
         \x20       const typedScore: int = score\n\
         \x20       out = $\"{out}rv{score};\"\n\
         \x20   for playerId, score in entries(scores)\n\
         \x20       const typedPlayer: string = playerId\n\
         \x20       const typedScore: int = score\n\
         \x20       out = $\"{out}e{playerId}={score};\"\n\
         \x20   for playerId, score in reversed(entries(scores))\n\
         \x20       const typedPlayer: string = playerId\n\
         \x20       const typedScore: int = score\n\
         \x20       out = $\"{out}re{playerId}={score};\"\n\
         \x20   return out\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str(
            "v10;v20;rv20;rv10;ep1=10;ep2=20;rep2=20;rep1=10;".into()
        ))
    );
}

#[test]
fn double_reversed_local_keyed_map_is_the_identity() {
    // `reversed(reversed(x)) == x`: re-reversing restores ascending order and keeps
    // every key paired with its own value. A single reverse stays descending pairs.
    let program = checked_program(
        "pub fn keyed(): string\n\
         \x20   var scores(player: string): int\n\
         \x20   scores(\"amy\") = 10\n\
         \x20   scores(\"bob\") = 20\n\
         \x20   var one: string = \"\"\n\
         \x20   for k, v in reversed(scores)\n\
         \x20       one = $\"{one}{k}={v};\"\n\
         \x20   var two: string = \"\"\n\
         \x20   for k, v in reversed(reversed(scores))\n\
         \x20       two = $\"{two}{k}={v};\"\n\
         \x20   var twoKeys: string = \"\"\n\
         \x20   for k in reversed(reversed(scores))\n\
         \x20       twoKeys = $\"{twoKeys}{k};\"\n\
         \x20   return $\"one:{one}two:{two}keys:{twoKeys}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed")).unwrap(),
        Some(Value::Str(
            "one:bob=20;amy=10;two:amy=10;bob=20;keys:amy;bob;".into()
        ))
    );
}

#[test]
fn double_reversed_local_sequence_is_the_identity() {
    // A sequence re-reversed restores ascending positions and elements. The single
    // reverse stays descending; the value view reverses elements directly.
    let program = checked_program(
        "pub fn seq(): string\n\
         \x20   var xs: sequence[int]\n\
         \x20   append(xs, 100)\n\
         \x20   append(xs, 200)\n\
         \x20   append(xs, 300)\n\
         \x20   var one: string = \"\"\n\
         \x20   for p in reversed(xs)\n\
         \x20       one = $\"{one}{p};\"\n\
         \x20   var twoPos: string = \"\"\n\
         \x20   for p in reversed(reversed(xs))\n\
         \x20       twoPos = $\"{twoPos}{p};\"\n\
         \x20   var twoPairs: string = \"\"\n\
         \x20   for p, v in reversed(reversed(xs))\n\
         \x20       twoPairs = $\"{twoPairs}{p}={v};\"\n\
         \x20   return $\"one:{one}pos:{twoPos}pairs:{twoPairs}\"\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::seq")).unwrap(),
        Some(Value::Str(
            "one:3;2;1;pos:1;2;3;pairs:1=100;2=200;3=300;".into()
        ))
    );
}

#[test]
fn reads_and_writes_a_multi_key_local_tree() {
    let program = checked_program(
        "pub fn keyed(day: date): int\n\
         \x20   var counts(day: date, category: string): int\n\
         \x20   counts(day, \"open\") = 3\n\
         \x20   return counts(day, \"open\") ?? -1\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::keyed", Value::Date(1))).unwrap(),
        Some(Value::Int(3))
    );
}

#[test]
fn std_math_decimal_helpers() {
    // absDecimal yields a decimal; floor rounds toward negative infinity to an int.
    let program = checked_program(
        "pub fn a(): string\n    return $\"{std::math::absDecimal(-2.5)}\"\n\n\
         pub fn up(): int\n    return std::math::floor(2.7)\n\n\
         pub fn down(): int\n    return std::math::floor(-2.7)\n",
    );
    assert_eq!(
        run(checked_entry!(&program, "test::a")).unwrap(),
        Some(Value::Str("2.5".into()))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::up")).unwrap(),
        Some(Value::Int(2))
    );
    assert_eq!(
        run(checked_entry!(&program, "test::down")).unwrap(),
        Some(Value::Int(-3))
    );
}
