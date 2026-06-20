use crate::support;
use marrow_check::DiagnosticPayload;
use support::{assert_clean, check_module, check_module_report, with_code};

fn codes(report: &marrow_check::CheckReport) -> Vec<&str> {
    report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

#[test]
fn a_nested_group_field_read_resolves_its_type() {
    // A read through nested group layers resolves to the innermost field's type,
    // so a typed return of it is not flagged as an untyped value.
    let found = check_module(
        "nested-read",
        "module m\n\
         resource Book\n    required title: string\n    \
         versions(version: int)\n        required title: string\n        \
         comments(pos: int)\n            required text: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    return ^books(1).versions(2).comments(3).text\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_loop_over_an_undeclared_index_is_a_collection_error_not_a_key_type_error() {
    // `^books.byShelf("fiction")` calls a member that is not a declared index. The
    // root cause is the missing index, so the diagnostic is the collection-unsupported
    // code carrying the index it would take to admit the lookup — never the
    // `check.key_type` "address it with an identity" error, which describes a
    // different mistake.
    let report = check_module_report(
        "loop-undeclared-index",
        "module m\n\
         resource Book\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn f(shelf: string)\n    for id in ^books.byShelf(shelf)\n        print(id)\n",
    );

    let found = with_code(&report, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    let DiagnosticPayload::SuggestedIndex { declaration } = &found[0].payload else {
        panic!(
            "expected suggested index payload, got {:#?}",
            found[0].payload
        );
    };
    assert_eq!(declaration, "index byShelf(shelf, id)");
    assert!(
        with_code(&report, "check.key_type").is_empty(),
        "an undeclared-index lookup is not a key-type error: {:#?}",
        report.diagnostics
    );
}

/// The composite-key grid the descent tests address: a single leaf layer with two
/// key columns, modeled as a chain of single-key sub-layers.
const GRID_CELLS: &str = "module m\n\
     resource Grid\n    cells(row: int, col: int): string\n\
     store ^grids(id: int): Grid\n\n";

#[test]
fn a_two_name_loop_over_a_composite_leaf_layer_is_rejected_with_a_descent_diagnostic() {
    // The composite-direct two-var form addresses two key columns at once, which the
    // tuple-free navigation model does not support: iterate the outer key, then
    // descend `cells(outer)` for the inner. The checker rejects it at the iterable
    // span rather than letting it check clean and fault `run.absent_element`.
    let src = format!(
        "{GRID_CELLS}fn f()\n    for row, col in ^grids(1).cells\n        print($\"{{row}},{{col}}\")\n"
    );
    let report = check_module_report("composite-two-name-loop", &src);

    let found = with_code(&report, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    let span = found[0].span;
    assert_eq!(&src[span.start_byte..span.end_byte], "^grids(1).cells");

    assert!(
        with_code(&report, "check.key_type").is_empty(),
        "the composite two-name loop is a collection-shape error, not a key-type error: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_range_leaving_a_further_column_emits_one_precise_diagnostic() {
    // `cells(lo..hi)` ranges the outer `row` column and leaves `col` unfilled, which
    // a ranged key argument may not. That arity error is the precise root cause and
    // owns the rejection alone — the path is neither a single value nor an iterable,
    // so the collection-shape check must not pile a second diagnostic on the span.
    let src = format!(
        "{GRID_CELLS}fn f(lo: int, hi: int)\n    for col in ^grids(1).cells(lo..hi)\n        print($\"{{col}}\")\n"
    );
    let report = check_module_report("range-leaves-column", &src);
    let found = with_code(&report, "check.key_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        with_code(&report, "check.collection_unsupported").is_empty(),
        "the range-arity error owns the rejection, not a secondary single-value error: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_partial_key_descent_types_the_inner_key_and_leaf() {
    // `cells(row)` descends to the inner `col -> string` sub-layer. A single-name loop
    // binds `col` as `int`; a two-name loop binds `col` and the leaf `string`. Typing
    // each into the wrong scalar proves the inner shape resolves rather than staying
    // `unknown`.
    let inner_key = format!(
        "{GRID_CELLS}fn f()\n    for col in ^grids(1).cells(1)\n        const c: bool = col\n"
    );
    let found = check_module("descent-inner-key", &inner_key, "check.assignment_type");
    assert_eq!(found.len(), 1, "{found:#?}");

    let inner_value = format!(
        "{GRID_CELLS}fn f()\n    for col, v in ^grids(1).cells(1)\n        const c: bool = v\n"
    );
    let found = check_module("descent-inner-value", &inner_value, "check.assignment_type");
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_partial_key_descent_is_clean_in_loop_and_count_position() {
    // The descent forms the runtime executes must check clean: the inner loop, the
    // two-name inner loop, the full-key leaf read, and `count` over the sub-layer.
    let src = format!(
        "{GRID_CELLS}fn f()\n    \
         for col in ^grids(1).cells(1)\n        print($\"{{col}}\")\n    \
         for col, v in ^grids(1).cells(1)\n        print($\"{{col}}={{v}}\")\n    \
         const leaf: string = ^grids(1).cells(1, 2) ?? \"\"\n    \
         const n: int = count(^grids(1).cells(1))\n    print($\"{{leaf}} {{n}}\")\n"
    );
    assert_clean(&check_module_report("descent-clean", &src));
}

/// Assert `src` raises exactly one `check.layer_not_value` whose span underlines the
/// whole partial-key access `access` and whose payload names the partial layer `layer`.
fn partial_key_value_rejected(name: &str, src: &str, access: &str, layer: &str) {
    let report = check_module_report(name, src);
    let found = with_code(&report, "check.layer_not_value");
    assert_eq!(found.len(), 1, "{name}: {:#?}", report.diagnostics);
    let span = found[0].span;
    assert_eq!(
        &src[span.start_byte..span.end_byte],
        access,
        "{name}: span should underline `{access}`: {:#?}",
        found[0]
    );
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::LayerNotValue {
            field: layer.to_string()
        },
        "{name}: {:#?}",
        found[0].payload
    );
}

#[test]
fn a_partial_key_in_a_bare_value_read_is_rejected_not_faulted() {
    // A one-remaining-column composite layer in a bare value-read position — a scalar
    // bind without `??`, a string interpolation, a plain call argument, or a function
    // return — names an iterable inner sub-layer, never a scalar. Each must be a clean
    // check error rather than the check-clean-then-`run.absent_element` fault that a
    // value-typed partial key produces. The diagnostic underlines the whole access and
    // names the partial `cells` layer in its payload.
    let cases = [
        (
            "bare-scalar-bind",
            format!("{GRID_CELLS}fn f()\n    const c: string = ^grids(1).cells(1)\n    print(c)\n"),
        ),
        (
            "bare-interpolation",
            format!("{GRID_CELLS}fn f()\n    print($\"{{^grids(1).cells(1)}}\")\n"),
        ),
        (
            "bare-call-argument",
            format!(
                "{GRID_CELLS}fn takes(s: string)\n    print(s)\n\
                 fn f()\n    takes(^grids(1).cells(1))\n"
            ),
        ),
        (
            "bare-return",
            format!("{GRID_CELLS}fn f(): string\n    return ^grids(1).cells(1)\n"),
        ),
    ];
    for (name, src) in &cases {
        partial_key_value_rejected(name, src, "^grids(1).cells(1)", "cells");
    }
}

#[test]
fn an_if_const_subject_of_a_partial_composite_layer_emits_one_layer_not_value() {
    // `if const c = ^grids(1).cells(1)` binds a partial composite layer as its subject.
    // The subject is not a bindable saved value read, but the precise partial-key
    // diagnostic is the single root cause: the generic "requires a saved value read"
    // check must suppress its cascade once `check.layer_not_value` is recorded on the
    // subject span, so exactly one diagnostic fires.
    let src =
        format!("{GRID_CELLS}fn f()\n    if const c = ^grids(1).cells(1)\n        print(c)\n");
    let report = check_module_report("if-const-partial-composite", &src);
    assert_eq!(
        codes(&report),
        vec!["check.layer_not_value"],
        "{:#?}",
        report.diagnostics
    );
    let span = report.diagnostics[0].span;
    assert_eq!(&src[span.start_byte..span.end_byte], "^grids(1).cells(1)");
}

#[test]
fn a_non_saved_read_if_const_subject_still_reports_condition_type() {
    // The suppression is narrow: an `if const` subject that is not a saved value read
    // at all (a plain local) carries no `check.layer_not_value`, so the generic
    // "requires a saved value read" diagnostic must still fire.
    let src = format!(
        "{GRID_CELLS}fn f()\n    const x: int = 1\n    if const c = x\n        print($\"{{c}}\")\n"
    );
    let report = check_module_report("if-const-local-subject", &src);
    let found = with_code(&report, "check.condition_type");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    assert!(
        with_code(&report, "check.layer_not_value").is_empty(),
        "a plain local subject is not a partial-key descent: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_zero_arg_bare_composite_layer_in_a_value_read_is_rejected_not_faulted() {
    // The maximal-partial boundary: `^grids(1).cells` with NO key columns filled is a
    // field access, not a call, so it reaches the value-read gate through the
    // field-access path rather than the call path. A bare composite layer there names
    // the whole iterable inner sub-layer, never a scalar, so every value position must
    // reject it at check rather than let an untyped value leak through interpolation —
    // which imposes no type expectation — and fault `run.unsupported` at runtime.
    let cases = [
        (
            "zero-arg-scalar-bind",
            format!("{GRID_CELLS}fn f()\n    const c: string = ^grids(1).cells\n    print(c)\n"),
        ),
        (
            "zero-arg-interpolation",
            format!("{GRID_CELLS}fn f()\n    print($\"{{^grids(1).cells}}\")\n"),
        ),
        (
            "zero-arg-call-argument",
            format!(
                "{GRID_CELLS}fn takes(s: string)\n    print(s)\n\
                 fn f()\n    takes(^grids(1).cells)\n"
            ),
        ),
        (
            "zero-arg-return",
            format!("{GRID_CELLS}fn f(): string\n    return ^grids(1).cells\n"),
        ),
    ];
    for (name, src) in &cases {
        partial_key_value_rejected(name, src, "^grids(1).cells", "cells");
    }

    // The three-key cube at its maximal-partial boundary (`^cubes(1).cells`) is the
    // same leak through the field-access value path.
    let cube_src = format!("{CUBE_CELLS}fn f()\n    print($\"{{^cubes(1).cells}}\")\n");
    partial_key_value_rejected("zero-arg-cube", &cube_src, "^cubes(1).cells", "cells");
}

#[test]
fn a_two_remaining_column_partial_key_in_a_bare_value_read_is_rejected() {
    // Two columns still unfilled (`vals(1)` on `vals(x, y, z)`) is the same leak at the
    // other boundary. Before the strict gate this typed as `unknown` and surfaced only
    // a generic `untyped_value`; now the precise partial-key diagnostic owns it.
    let cube = "module m\n\
         resource Cube\n    vals(x: int, y: int, z: int): string\n\
         store ^cubes(id: int): Cube\n\n";
    let cases = [
        (
            "cube-scalar-bind",
            format!("{cube}fn f()\n    const c: string = ^cubes(1).vals(1)\n    print(c)\n"),
        ),
        (
            "cube-return",
            format!("{cube}fn f(): string\n    return ^cubes(1).vals(1)\n"),
        ),
    ];
    for (name, src) in &cases {
        partial_key_value_rejected(name, src, "^cubes(1).vals(1)", "vals");
    }
}

#[test]
fn a_write_to_a_partial_key_layer_is_rejected_at_check() {
    // Assigning to a partially keyed layer names an inner sub-layer, not a writable
    // entry, so it is a check error rather than a `write.layer_key_arity` fault. The
    // invalid-target rejection is the single root cause: a write target is not a value
    // read, so the value-position partial-key gate must not stack a second
    // `check.layer_not_value` on the same span.
    let src = format!("{GRID_CELLS}fn f()\n    ^grids(1).cells(1) = \"nope\"\n");
    let report = check_module_report("descent-partial-write", &src);
    assert_eq!(
        codes(&report),
        vec!["check.invalid_assign_target"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_delete_of_a_partial_key_layer_is_rejected_at_check() {
    // `delete ^grids(1).cells(1)` supplies only the outer `row` column, so the
    // address names an iterable inner sub-layer, not a deletable entry. Accepting it
    // would cascade-delete every `col` stored under that `row`; the partial-key guard
    // rejects it at check, the same as a partial-key write, never lowering a delete.
    let src = format!("{GRID_CELLS}fn f()\n    delete ^grids(1).cells(1)\n");
    let report = check_module_report("delete-partial-2key", &src);
    let found = with_code(&report, "check.invalid_assign_target");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    let span = found[0].span;
    assert_eq!(&src[span.start_byte..span.end_byte], "^grids(1).cells(1)");
    // A delete target is not a value read; the invalid-target rejection owns the span
    // alone, with no stacked value-position `check.layer_not_value`.
    assert!(
        with_code(&report, "check.layer_not_value").is_empty(),
        "a partial-key delete must not cascade a value-read error: {:#?}",
        report.diagnostics
    );
}

#[test]
fn a_delete_of_a_partial_three_key_layer_is_rejected_at_check() {
    // Dropping one or two of three columns is the same cascade risk: each names a
    // sub-layer, not a deletable entry. Both partial arities are rejected at check.
    for (name, target) in [
        ("delete-partial-3key-one", "^cubes(1).vals(1)"),
        ("delete-partial-3key-two", "^cubes(1).vals(1, 2)"),
    ] {
        let src = format!(
            "module m\n\
             resource Cube\n    vals(x: int, y: int, z: int): string\n\
             store ^cubes(id: int): Cube\n\n\
             fn f()\n    delete {target}\n"
        );
        let report = check_module_report(name, &src);
        let found = with_code(&report, "check.invalid_assign_target");
        assert_eq!(found.len(), 1, "{name}: {:#?}", report.diagnostics);
        assert!(
            with_code(&report, "check.layer_not_value").is_empty(),
            "{name}: a partial-key delete must not cascade a value-read error: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_delete_of_a_full_key_composite_leaf_stays_clean() {
    // Supplying every column reaches one leaf entry, the deletable address. The
    // partial-key guard must not sweep up a full-arity delete.
    let src = format!("{GRID_CELLS}fn f()\n    delete ^grids(1).cells(1, 2)\n");
    assert_clean(&check_module_report("delete-full-key-composite", &src));
}

#[test]
fn a_ranged_key_delete_is_rejected_at_check() {
    // A ranged key argument in a delete address has no single entry to remove; the
    // runtime cannot evaluate a range key in a delete, so a ranged layer/root delete
    // must be rejected at check exactly as a ranged assignment already is, never
    // lowering to a `run.unsupported` fault. This covers a single-key sequence and a
    // composite leaf whose final column is ranged.
    for (name, schema, target) in [
        (
            "delete-range-single-key",
            "module m\n\
             resource Doc\n    lines(n: int): string\n\
             store ^docs(id: int): Doc\n\n",
            "^docs(1).lines(1..2)",
        ),
        (
            "delete-range-composite-leaf",
            GRID_CELLS,
            "^grids(1).cells(1, 1..5)",
        ),
    ] {
        let src = format!("{schema}fn f()\n    delete {target}\n");
        let report = check_module_report(name, &src);
        assert_eq!(
            codes(&report),
            vec!["check.invalid_assign_target"],
            "{name}: {:#?}",
            report.diagnostics
        );
        let span = report.diagnostics[0].span;
        assert_eq!(
            &src[span.start_byte..span.end_byte],
            target,
            "{name}: span should underline `{target}`"
        );
    }
}

#[test]
fn a_ranged_index_branch_delete_is_rejected_at_check() {
    // A ranged index branch is already rejected by the index-branch delete guard,
    // which owns the span with `collection_unsupported`; the range gate must not stack
    // a second diagnostic on it.
    let src = "module m\n\
         resource Post\n    published: int\n\
         store ^posts(id: int): Post\n\n    index byDate(published, id)\n\n\
         fn f()\n    delete ^posts.byDate(1..5)\n";
    let report = check_module_report("delete-range-index", src);
    assert_eq!(
        codes(&report),
        vec!["check.collection_unsupported"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn append_to_a_composite_layer_is_rejected_at_check() {
    // `append` allocates a single integer position; a composite (multi-column) layer
    // is a chain of sub-layers with no single column to allocate into. Every shape —
    // the bare outer layer, a partial prefix, and the full leaf — is rejected at
    // check rather than left to fault `write.layer_key_arity` or `run.unsupported`.
    for (name, target) in [
        ("append-composite-outer", "^grids(1).cells"),
        ("append-composite-partial", "^grids(1).cells(1)"),
        ("append-composite-leaf", "^grids(1).cells(1, 2)"),
    ] {
        let src = format!("{GRID_CELLS}fn f()\n    append({target}, \"z\")\n");
        let report = check_module_report(name, &src);
        let found = with_code(&report, "check.call_argument");
        assert_eq!(found.len(), 1, "{name}: {:#?}", report.diagnostics);
        assert!(
            matches!(
                found[0].payload,
                DiagnosticPayload::AppendTarget(
                    marrow_check::AppendTargetDiagnostic::CompositeLayer
                )
            ),
            "{name}: {:#?}",
            found[0].payload
        );
        // The append target is a write-collection position, not a value read, so the
        // composite-layer rejection is the single root cause; the partial-key value
        // gate must not stack a second `check.layer_not_value` on it.
        assert!(
            with_code(&report, "check.layer_not_value").is_empty(),
            "{name}: append target must not cascade a value-read error: {:#?}",
            report.diagnostics
        );
    }
}

#[test]
fn a_partial_key_layer_appended_as_a_value_is_rejected() {
    // `append` reads its second argument as the value to store. A partially keyed
    // composite layer there is the same non-value misuse as any other value position,
    // so it is rejected on the value-read gate rather than typed as the leaf string.
    let src = "module m\n\
         resource Grid\n    tags(pos: int): string\n    \
         cells(row: int, col: int): string\n\
         store ^grids(id: int): Grid\n\n\
         fn f()\n    append(^grids(1).tags, ^grids(1).cells(1))\n";
    partial_key_value_rejected("append-value-partial", src, "^grids(1).cells(1)", "cells");
}

#[test]
fn append_to_a_single_column_int_layer_stays_clean() {
    // The one valid append target is a single int-keyed leaf layer (a sequence):
    // `append` allocates the next position in its only column. The composite
    // rejection must not sweep it up.
    let src = "module m\n\
         resource Doc\n    lines(n: int): string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n    append(^docs(1).lines, \"first\")\n";
    let found = check_module("append-single-int", src, "check.call_argument");
    assert!(found.is_empty(), "{found:#?}");
}

/// A three-key cube layer: a chain of three single-key sub-layers. Descending one
/// column (`cells(x)`) leaves a `y -> z -> string` sub-layer whose entries are
/// themselves sub-layers, so its value position is not a scalar leaf.
const CUBE_CELLS: &str = "module m\n\
     resource Cube\n    cells(x: int, y: int, z: int): string\n\
     store ^cubes(id: int): Cube\n\n";

#[test]
fn iterating_a_fully_keyed_leaf_value_is_rejected() {
    // `cells(1, 1)` addresses one stored string, not a collection. A `for` over it
    // names a single value with no key to stream, so the checker rejects it rather
    // than letting it check clean and fault `run.unsupported`.
    let src = format!(
        "{GRID_CELLS}fn f()\n    for x in ^grids(1).cells(1, 1)\n        print($\"{{x}}\")\n"
    );
    let report = check_module_report("iterate-full-leaf", &src);
    let found = with_code(&report, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    let span = found[0].span;
    assert_eq!(
        &src[span.start_byte..span.end_byte],
        "^grids(1).cells(1, 1)"
    );
}

#[test]
fn iterating_a_saved_scalar_field_is_rejected() {
    // `^books(1).title` reads one stored scalar. Iterating it names a single value,
    // not a collection, so it is a clean check error, never a runtime fault.
    let src = "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    for c in ^books(1).title\n        print($\"{c}\")\n";
    let found = check_module("iterate-saved-scalar", src, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn iterating_a_single_key_full_leaf_read_is_rejected() {
    // `^books(1).versions(1)` addresses one stored leaf entry, not a collection.
    let src = "module m\n\
         resource Book\n    versions(v: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    for x in ^books(1).versions(1)\n        print($\"{x}\")\n";
    let found = check_module(
        "iterate-full-single-key",
        src,
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn iterating_a_whole_saved_record_is_rejected() {
    // `^grids(1)` reads one whole record. A record is a value, not a collection of
    // keys to stream, so a bare `for` over it is a clean check error.
    let src = format!("{GRID_CELLS}fn f()\n    for x in ^grids(1)\n        print($\"{{x}}\")\n");
    let found = check_module("iterate-whole-record", &src, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn iterating_a_singleton_root_is_rejected() {
    // A keyless singleton store has no identities to enumerate; `^settings` reads one
    // record value, so iterating it is a clean check error, not a runtime fault.
    let src = "module m\n\
         resource Settings\n    theme: string\n\
         store ^settings: Settings\n\n\
         fn f()\n    for s in ^settings\n        print(\"x\")\n";
    let found = check_module(
        "iterate-singleton-root",
        src,
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn values_and_entries_over_a_multi_column_partial_layer_are_rejected() {
    // `cells(x)` on a three-key cube leaves a two-column sub-layer whose value
    // position is itself a sub-layer, not a scalar. `values(...)` and `entries(...)`
    // pair a key with that sub-layer, so both are rejected at check, mirroring the
    // bare two-name loop. The same holds for the canonical two-column grid head.
    for (name, head) in [
        ("values-cube", "values(^cubes(1).cells(1))"),
        ("entries-grid", "entries(^grids(1).cells)"),
        ("values-grid", "values(^grids(1).cells)"),
    ] {
        let base = if head.contains("cubes") {
            CUBE_CELLS
        } else {
            GRID_CELLS
        };
        let binding = if head.starts_with("entries") {
            "row, v"
        } else {
            "v"
        };
        let src = format!("{base}fn f()\n    for {binding} in {head}\n        print($\"x\")\n");
        let found = check_module(name, &src, "check.collection_unsupported");
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn entries_over_a_two_column_partial_layer_is_rejected() {
    // `entries(^cubes(1).cells(1))` over a two-column-remaining sub-layer pairs the
    // inner `y` key with a `z -> string` sub-layer, not a leaf value.
    let src = format!(
        "{CUBE_CELLS}fn f()\n    for y, v in entries(^cubes(1).cells(1))\n        print($\"{{y}}={{v}}\")\n"
    );
    let found = check_module("entries-cube-partial", &src, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn keys_and_count_over_a_multi_column_partial_layer_stay_clean() {
    // `keys(...)` and `count(...)` operate on the next key column of a partial layer,
    // which is well-defined regardless of how many deeper columns remain. They must
    // not be swept up by the value-position rejection.
    let src = format!(
        "{CUBE_CELLS}fn f()\n    \
         for x in keys(^cubes(1).cells(1))\n        print($\"{{x}}\")\n    \
         const n: int = count(^cubes(1).cells(1))\n    print($\"{{n}}\")\n"
    );
    assert_clean(&check_module_report("keys-count-multi-column", &src));
}

#[test]
fn descending_one_column_at_a_time_stays_clean() {
    // The descent forms the runtime executes must remain clean: descend the cube one
    // column to a two-column sub-layer, descend again to the final `z -> string`
    // sub-layer, then iterate its keys, values, and entries.
    let src = format!(
        "{CUBE_CELLS}fn f()\n    \
         for x in ^cubes(1).cells\n        \
         for y in ^cubes(1).cells(x)\n            \
         for z, v in ^cubes(1).cells(x, y)\n                print($\"{{x}},{{y}},{{z}}={{v}}\")\n"
    );
    assert_clean(&check_module_report("cube-descent-clean", &src));

    let leaf_iters = format!(
        "{CUBE_CELLS}fn f(x: int, y: int)\n    \
         for z in ^cubes(1).cells(x, y)\n        print($\"{{z}}\")\n    \
         for v in values(^cubes(1).cells(x, y))\n        print($\"{{v}}\")\n    \
         for z, v in entries(^cubes(1).cells(x, y))\n        print($\"{{z}}={{v}}\")\n"
    );
    assert_clean(&check_module_report("cube-leaf-iters-clean", &leaf_iters));
}

/// A composite layer whose leaf is a record carrying its own keyed child layer.
/// Descending one column binds an `Inner` record; its keyed child layer `items`
/// is reachable only through a saved address, never through that materialized value.
const NESTED_LAYER_RECORD: &str = "module m\n\
     resource Inner\n    items(k: int): string\n    label: string\n\
     resource Outer\n    groups(row: int, col: int): Inner\n\
     store ^outers(id: int): Outer\n\n";

#[test]
fn a_keyed_child_layer_read_through_a_descent_bound_record_is_rejected() {
    // `for col, inner in ^outers(1).groups(row)` binds `inner` to a materialized
    // `Inner` record. Its keyed child layer `items` is not pulled into that value,
    // so `inner.items` is a check error, not an accepted-then-faulted read.
    let iterate = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int)\n    \
         for col, inner in ^outers(1).groups(row)\n        \
         for k in inner.items\n            print($\"{{col}} {{k}}\")\n"
    );
    let found = check_module(
        "descent-record-nested-iter",
        &iterate,
        "check.layer_not_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");

    let counted = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int)\n    \
         for col, inner in ^outers(1).groups(row)\n        \
         print($\"{{count(inner.items)}}\")\n"
    );
    let found = check_module(
        "descent-record-nested-count",
        &counted,
        "check.layer_not_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_scalar_field_of_a_descent_bound_record_still_resolves() {
    // The keyed-child-layer rejection must not touch plain fields: scalars are
    // materialized into the bound record, so `inner.label` types cleanly.
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int)\n    \
         for col, inner in ^outers(1).groups(row)\n        \
         const l: string = inner.label ?? \"\"\n        print($\"{{col}} {{l}}\")\n"
    );
    assert_clean(&check_module_report("descent-record-scalar", &src));
}

#[test]
fn a_keyed_child_layer_read_through_a_materialized_record_is_rejected() {
    // A whole-record read binds a materialized value that omits its keyed child
    // layers; iterating one through that value is a check error, not a runtime fault.
    let src = "module m\n\
         resource Book\n    versions(v: int): string\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    if const b = ^books(1)\n        \
         for v in b.versions\n            print($\"{v}\")\n";
    let found = check_module("materialized-keyed-layer", src, "check.layer_not_value");
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_keyed_child_layer_read_off_a_materialized_record_with_a_fallback_emits_one_error() {
    // `b.versions(2) ?? "x"` reads a keyed child layer off a materialized record, then
    // guards it with `??`. The layer-not-value descent is the single mistake; the
    // operator/untyped checks suppress their cascade once it is recorded on the span.
    let src = "module m\n\
         resource Book\n    versions(v: int): string\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    if const b = ^books(1)\n        \
         return b.versions(2) ?? \"x\"\n    return \"\"\n";
    let report = check_module_report("materialized-keyed-layer-fallback", src);
    assert_eq!(
        codes(&report),
        vec!["check.layer_not_value"],
        "{:#?}",
        report.diagnostics
    );
}

/// A three-key composite layer whose leaf is a record carrying its own keyed child
/// layer, the same shape as [`NESTED_LAYER_RECORD`] with one more outer column.
const NESTED_LAYER_RECORD_3KEY: &str = "module m\n\
     resource Inner\n    items(k: int): string\n    label: string\n\
     resource Cell\n    grid(x: int, y: int, z: int): Inner\n\
     store ^cells(id: int): Cell\n\n";

/// A single-key entry-resource child-layer descent (`^outers(1).group(row).items`)
/// addresses a real leaf entry, so its child layer is reachable through the saved
/// address. The composite-partial rejection must not sweep it up.
const SINGLE_KEY_ENTRY_LAYER: &str = "module m\n\
     resource Inner\n    items(k: int): string\n    label: string\n\
     resource Outer\n    group(row: int): Inner\n\
     store ^outers(id: int): Outer\n\n";

fn descent_layer_not_value(report: &marrow_check::CheckReport, src: &str, field: &str) {
    let found = with_code(report, "check.layer_not_value");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
    // The diagnostic underlines the rejected field access and ends at the descended
    // field name; the typed payload carries that field as the structured identity.
    let span = found[0].span;
    assert!(
        src[span.start_byte..span.end_byte].ends_with(field),
        "span should end at `{field}`: {:#?}",
        found[0]
    );
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::LayerNotValue {
            field: field.to_string()
        },
        "{:#?}",
        found[0].payload
    );
}

#[test]
fn writing_a_child_layer_off_a_partial_composite_layer_is_rejected() {
    // `^outers(1).groups(row).items(k) = txt` supplies only `row` of the composite
    // `(row, col)` key, so `groups(row)` names an inner sub-layer to descend, not a
    // record value. Descending `.items` off it would write durable data at a phantom
    // address with `col` silently elided; the descent is a check error on the field.
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, k: int, txt: string)\n    \
         ^outers(1).groups(row).items(k) = txt\n"
    );
    let report = check_module_report("descent-partial-write-2key", &src);
    descent_layer_not_value(&report, &src, "items");
    // The phantom write never lowers: the partial-prefix descent produces no saved
    // place, so no write effect is recorded against `^outers`.
    assert!(
        with_code(&report, "check.invalid_assign_target").is_empty(),
        "the descent error owns the rejection, not a write-target fallback: {:#?}",
        report.diagnostics
    );
}

#[test]
fn writing_a_child_layer_off_a_partial_three_key_layer_is_rejected() {
    // Dropping two of three columns (`grid(x)` of `grid(x, y, z)`) is the same phantom
    // write: `.items` descends off a sub-layer, not a record value.
    let src = format!(
        "{NESTED_LAYER_RECORD_3KEY}fn f(x: int, k: int, txt: string)\n    \
         ^cells(1).grid(x).items(k) = txt\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-write-3key", &src),
        &src,
        "items",
    );

    let src = format!(
        "{NESTED_LAYER_RECORD_3KEY}fn f(x: int, y: int, k: int, txt: string)\n    \
         ^cells(1).grid(x, y).items(k) = txt\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-write-3key-two", &src),
        &src,
        "items",
    );
}

#[test]
fn reading_a_child_layer_off_a_partial_composite_layer_is_rejected() {
    // A guarded read of the phantom descent address is the same error: the base names
    // a sub-layer, so no leaf value is reachable.
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, k: int): string\n    \
         return ^outers(1).groups(row).items(k) ?? \"\"\n"
    );
    let report = check_module_report("descent-partial-read", &src);
    descent_layer_not_value(&report, &src, "items");
    // The descent error owns the mistake; the `??` over the descended value must not
    // pile a second `check.bare_maybe_present_read` on the same span.
    assert_eq!(
        codes(&report),
        vec!["check.layer_not_value"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_scalar_field_off_a_partial_composite_layer_with_a_fallback_emits_one_error() {
    // A `.field` descent off a partial composite layer guarded by `??` is a single
    // mistake: the layer-not-value descent. The downstream presence/untyped checks
    // suppress their cascade on that span, so exactly one diagnostic fires.
    let src = "module m\n\
         resource Grid\n    cells(row: int, col: int)\n        required note: string\n\
         store ^grids(id: int): Grid\n\n\
         fn f(): string\n    return ^grids(1).cells(5).note ?? \"d\"\n";
    let report = check_module_report("descent-partial-field-fallback", src);
    assert_eq!(
        codes(&report),
        vec!["check.layer_not_value"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn iterating_a_child_layer_off_a_partial_composite_layer_is_rejected() {
    // `for k in ^outers(1).groups(row).items` streams a phantom inner layer; the
    // descent is rejected before any iteration shape is considered.
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int)\n    \
         for k in ^outers(1).groups(row).items\n        print($\"{{k}}\")\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-for", &src),
        &src,
        "items",
    );
}

#[test]
fn counting_a_child_layer_off_a_partial_composite_layer_is_rejected() {
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int): int\n    \
         return count(^outers(1).groups(row).items)\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-count", &src),
        &src,
        "items",
    );
}

#[test]
fn existence_of_a_child_layer_off_a_partial_composite_layer_is_rejected() {
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, k: int): bool\n    \
         return exists(^outers(1).groups(row).items(k))\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-exists", &src),
        &src,
        "items",
    );
}

#[test]
fn deleting_a_child_layer_off_a_partial_composite_layer_is_rejected() {
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, k: int)\n    \
         delete ^outers(1).groups(row).items(k)\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-delete", &src),
        &src,
        "items",
    );
}

#[test]
fn a_single_key_entry_child_layer_descent_resolves() {
    // A single-key entry layer (`group(row)`) addresses a real leaf record, so its
    // child layer `items` is reachable through the saved address. Tightening the
    // composite-partial rejection must not touch it.
    let read = format!(
        "{SINGLE_KEY_ENTRY_LAYER}fn f(row: int, k: int): string\n    \
         return ^outers(1).group(row).items(k) ?? \"\"\n"
    );
    assert_clean(&check_module_report("single-key-entry-descent-read", &read));

    let write = format!(
        "{SINGLE_KEY_ENTRY_LAYER}fn f(row: int, k: int, txt: string)\n    \
         ^outers(1).group(row).items(k) = txt\n"
    );
    assert_clean(&check_module_report(
        "single-key-entry-descent-write",
        &write,
    ));

    let iter = format!(
        "{SINGLE_KEY_ENTRY_LAYER}fn f(row: int)\n    \
         for k in ^outers(1).group(row).items\n        print($\"{{k}}\")\n"
    );
    assert_clean(&check_module_report("single-key-entry-descent-for", &iter));
}

#[test]
fn a_full_key_composite_child_layer_descent_resolves() {
    // Supplying every composite column (`groups(row, col)`) reaches the leaf `Inner`
    // record, so descending its child layer `items` resolves through the saved
    // address under read, write, and iteration.
    let read = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, col: int, k: int): string\n    \
         return ^outers(1).groups(row, col).items(k) ?? \"\"\n"
    );
    assert_clean(&check_module_report(
        "full-key-composite-descent-read",
        &read,
    ));

    let write = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, col: int, k: int, txt: string)\n    \
         ^outers(1).groups(row, col).items(k) = txt\n"
    );
    assert_clean(&check_module_report(
        "full-key-composite-descent-write",
        &write,
    ));

    let iter = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int, col: int)\n    \
         for k in ^outers(1).groups(row, col).items\n        print($\"{{k}}\")\n"
    );
    assert_clean(&check_module_report(
        "full-key-composite-descent-for",
        &iter,
    ));
}

#[test]
fn a_scalar_field_off_a_partial_composite_layer_is_rejected() {
    // `groups(row).label` descends a plain field off a partial composite layer. The
    // base still names a sub-layer, not a record value, so the scalar field is not
    // reachable through it either — the same descent error owns it.
    let src = format!(
        "{NESTED_LAYER_RECORD}fn f(row: int): string\n    \
         return ^outers(1).groups(row).label ?? \"\"\n"
    );
    descent_layer_not_value(
        &check_module_report("descent-partial-scalar-field", &src),
        &src,
        "label",
    );
}

#[test]
fn a_nested_group_field_read_of_the_wrong_type_is_flagged() {
    // The nested read resolves to `string`, so storing it into an `int` is a
    // genuine type mismatch — proving the type is resolved, not left unknown.
    let found = check_module(
        "nested-read-mismatch",
        "module m\n\
         resource Book\n    required title: string\n    \
         versions(version: int)\n        required title: string\n        \
         comments(pos: int)\n            required text: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    const n: int = ^books(1).versions(2).comments(3).text\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_field_read_feeds_the_return_type_check() {
    // `^books(1).title` is `string` from the schema, but `f` returns `int`.
    let found = check_module(
        "saved-field-return",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return ^books(1).title\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_field_read_feeds_operator_checks() {
    // `currentVersion` is `int` from the schema, so `+ true` is int-plus-bool.
    let found = check_module(
        "saved-field-op",
        "module m\n\
         resource Book\n    currentVersion: int\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var x = ^books(1).currentVersion + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_binary_operator_over_a_saved_collection_is_a_check_error() {
    // A saved store root is an in-place stream with no materialized value, so it can
    // never be a binary operand. `count(^books + ^books)` once checked clean because a
    // saved-collection operand infers `Unknown`, deferring the operator check; the
    // operand rule now rejects it at the operator as a `check.operator_type` rather than
    // letting it fault clean-then-runtime.
    let found = check_module(
        "saved-collection-operand",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return count(^books + ^books)\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn binding_a_saved_collection_operator_result_is_a_check_error() {
    // The same operand rule fires when the result is bound to a local: the saved
    // collection is rejected at the operator, so no laundered value reaches the runtime.
    let found = check_module(
        "saved-collection-operand-bind",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn g()\n    var x = ^books + ^books\n    print($\"{count(x)}\")\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_collection_on_one_operator_side_is_a_check_error() {
    // The rejection fires when either side is a saved collection, not only both: a saved
    // root added to a local sequence is still a saved collection in operator position.
    let found = check_module(
        "saved-collection-operand-one-side",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn h(): int\n    var xs = std::text::split(\"a,b\", \",\")\n    return count(^books + xs)\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_keys_combinator_as_an_operator_operand_is_a_check_error() {
    // `keys(^books)` is a saved stream laundered through a combinator; as an operator
    // operand it is the same un-materializable saved collection the bare root is.
    let found = check_module(
        "saved-keys-operand",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return count(keys(^books) + keys(^books))\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_scalar_operand_stays_legal() {
    // A saved scalar read is a single stored value, not a collection, so it is a valid
    // operator operand: `^books(1).price + 1` must still check clean.
    let report = check_module_report(
        "saved-scalar-operand-ok",
        "module m\n\
         resource Book\n    price: int\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return (^books(1).price ?? 0) + 1\n",
    );
    assert_clean(&report);
}

#[test]
fn a_saved_collection_as_a_comparison_operand_is_a_check_error() {
    // A comparison is a binary operator like any other; a saved collection cannot be one
    // of its operands. The operand rule rejects it before the comparison's own typing.
    let found = check_module(
        "saved-collection-comparison",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\
         store ^others(id: int): Book\n\n\
         fn f(): bool\n    return ^books == ^others\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_saved_collection_as_a_coalesce_operand_is_a_check_error() {
    // `??` defaults an absent path read; a saved collection is a stream, not an absent
    // scalar, so it is not a coalesce subject. It is rejected as an operator operand.
    let found = check_module(
        "saved-collection-coalesce",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return count(^books ?? ^books)\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn printing_a_saved_collection_is_a_check_error() {
    // A saved collection is an in-place stream with no text form, so it cannot be a
    // print value. The render surface rejects it at check rather than faulting at run.
    let found = check_module(
        "print-saved-collection",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    print(^books)\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn interpolating_a_saved_collection_is_a_check_error() {
    // String interpolation is the same render surface as `print`: a saved collection
    // has no text form there either.
    let found = check_module(
        "interp-saved-collection",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    return $\"{^books}\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn interpolating_a_saved_scalar_is_not_flagged() {
    // A saved scalar read is a single stored value with a text form, so interpolating
    // it stays legal — the render rejection must not over-reach to saved scalars.
    let report = check_module_report(
        "interp-saved-scalar-ok",
        "module m\n\
         resource Book\n    pages: int\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books)): string\n    return $\"{^books(id).pages ?? 0}\"\n",
    );
    assert_clean(&report);
}

#[test]
fn an_unknown_saved_path_field_is_flagged() {
    let report = check_module_report(
        "saved-field-unknown",
        "module m\n\
         resource Thing\n    title: string\n\
         store ^things(id: int): Thing\n\n\
         fn f(id: Id(^things))\n    var x = ^things(id).nosuchfield\n",
    );
    assert_eq!(
        codes(&report),
        vec!["check.unknown_field"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_saved_path_field_suppresses_coalesce_noise() {
    let report = check_module_report(
        "saved-field-unknown-coalesce",
        "module m\n\
         resource Thing\n    title: string\n\
         store ^things(id: int): Thing\n\n\
         fn f(id: Id(^things)): string\n    return ^things(id).nosuchfield ?? \"fallback\"\n",
    );
    assert_eq!(
        codes(&report),
        vec!["check.unknown_field"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_correctly_typed_saved_field_read_is_not_flagged() {
    // `^books(1).title` is `string`, matching `f`'s declared `string` return.
    let found = check_module(
        "saved-field-ok",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    return ^books(1).title\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_local_resource_field_read_feeds_operator_checks() {
    // `book.title` is `string` from Book's schema, so `+ 1` is string-plus-int.
    let found = check_module(
        "local-field-op",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var book: Book\n    var x = book.title + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_local_resource_field_is_not_flagged() {
    // `book.title` is `string`, matching `f`'s declared `string` return.
    let found = check_module(
        "local-field-ok",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): string\n    var book: Book\n    return book.title\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_local_resource_field_is_flagged() {
    let report = check_module_report(
        "local-field-unknown",
        "module m\n\
         resource Book\n    title: string\n\
         fn f(b: Book)\n    var x = b.typoField\n",
    );
    assert_eq!(
        codes(&report),
        vec!["check.unknown_field"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_local_resource_field_suppresses_untyped_noise() {
    let report = check_module_report(
        "local-field-unknown-typed",
        "module m\n\
         resource Book\n    title: string\n\
         fn f(b: Book)\n    const x: string = b.typoField\n",
    );
    assert_eq!(
        codes(&report),
        vec!["check.unknown_field"],
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_base_field_read_does_not_report_unknown_field() {
    let report = check_module_report(
        "unknown-base-field",
        "module m\n\
         fn f(raw: unknown)\n    var x = raw.nosuchfield\n",
    );
    assert!(report.diagnostics.is_empty(), "{:#?}", report.diagnostics);
}

#[test]
fn a_whole_resource_read_into_a_local_types_its_fields() {
    // `^books(1)` reads the whole record as a `Book`; `b.title` then resolves to
    // `string` from the schema, so `+ 1` is string-plus-int.
    let found = check_module(
        "whole-read-field",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var b = ^books(1)\n    var x = b.title + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_local_resource_field_typed_as_a_resource_keeps_its_resource_shape() {
    let found = check_module(
        "local-resource-field-resource-type",
        "module m\n\
         resource Address\n    city: string\n\n\
         resource Person\n    address: Address\n\n\
         fn f()\n    var person = Person(address: Address(city: \"Paris\"))\n    var x = person.address.city + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn type_surface_ledger_reads_and_traversals_have_concrete_types() {
    let report = check_module_report(
        "ledger-type-surfaces",
        "module m\n\
         resource Account\n    required name: string\n    amounts(pos: int): decimal\n\
         store ^accounts(code: string): Account\n\n\
         fn sumAmounts(code: Id(^accounts)): decimal\n    var sum: decimal = 0.0\n    for amount in values(^accounts(code).amounts)\n        sum = sum + amount\n    return sum\n\n\
         fn countAccounts(): int\n    return count(^accounts)\n\n\
         fn ids()\n    for code in keys(^accounts)\n        const typed: Id(^accounts) = code\n\n\
         fn accounts()\n    for code, account in ^accounts\n        const name: string = account.name\n\n\
         fn handle(): bool\n    try\n        throw Error(code: \"x.y\", message: \"m\")\n    catch err: Error\n        return err.code == ErrorCode(\"x.y\")\n",
    );
    assert_clean(&report);
}

#[test]
fn a_group_field_read_feeds_type_checks() {
    // `^books(1).versions(2).title` is `string` from the group schema, but `f`
    // returns `int`.
    let found = check_module(
        "saved-group-field",
        "module m\n\
         resource Book\n    versions(v: int)\n        title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return ^books(1).versions(2).title\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_singleton_field_read_feeds_type_checks() {
    // `^settings.theme` on a keyless singleton store
    // is `string` from the schema, not Unknown — so a typed use never
    // false-positives check.untyped_value, and a real mismatch (returning it
    // from an `int` function) is caught.
    let found = check_module(
        "singleton-field",
        "module m\n\
         resource Settings\n    theme: string\n\
         store ^settings: Settings\n\n\
         fn f(): int\n    return ^settings.theme\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_singleton_field_read_in_a_typed_place_is_not_an_untyped_value() {
    // The documented `const t: string = ^settings.theme` reads a singleton field
    // into a matching place — no false check.untyped_value.
    let found = check_module(
        "singleton-field-ok",
        "module m\n\
         resource Settings\n    theme: string\n\
         store ^settings: Settings\n\n\
         fn f()\n    const t: string = ^settings.theme\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_singleton_keyed_leaf_read_feeds_type_checks() {
    let found = check_module(
        "singleton-keyed-leaf",
        "module m\n\
         resource Settings\n    counts(name: string): int\n\
         store ^settings: Settings\n\n\
         fn f(name: string): int\n    return ^settings.counts(name)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_singleton_keyed_group_field_read_feeds_type_checks() {
    let found = check_module(
        "singleton-keyed-group-field",
        "module m\n\
         resource Settings\n    tokens(pos: int)\n        kind: string\n\
         store ^settings: Settings\n\n\
         fn f(pos: int): string\n    return ^settings.tokens(pos).kind\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_singleton_whole_read_requires_read_site_resolution() {
    let report = check_module_report(
        "singleton-whole",
        "module m\n\
         resource Settings\n    theme: string\n    required maxLoans: int\n\
         store ^settings: Settings\n\n\
         fn snapshot(): Settings\n    return ^settings\n\n\
         fn restore(s: Settings)\n    ^settings = s\n",
    );
    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn an_unkeyed_group_field_read_feeds_type_checks() {
    // `^patients(1).name.first` reaches a scalar field through an unkeyed group
    // (`name { first; last }`). It is `string` from the schema, not Unknown, so a
    // typed mismatch (returning it from an `int` function) is caught.
    let found = check_module(
        "unkeyed-group-field",
        "module m\n\
         resource Patient\n\
         \x20   name\n        first: string\n        last: string\n\
         store ^patients(id: int): Patient\n\n\
         fn f(): int\n    return ^patients(1).name.first\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_unkeyed_group_field_read_is_not_flagged() {
    let found = check_module(
        "unkeyed-group-field-ok",
        "module m\n\
         resource Patient\n\
         \x20   name\n        first: string\n        last: string\n\
         store ^patients(id: int): Patient\n\n\
         fn f(): string\n    return ^patients(1).name.first\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_optional_group_field_read_preserves_the_leaf_type() {
    let found = check_module(
        "optional-group-field",
        "module m\n\
         resource Book\n\
         \x20   binding\n        cover: string\n\
         store ^books(id: int): Book\n\n\
         fn cover(id: Id(^books)): string\n    return ^books(id)?.binding?.cover\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_optional_keyed_root_chain_is_not_a_typed_leaf() {
    let found = check_module(
        "optional-keyed-root-chain",
        "module m\n\
         resource Book\n\
         \x20   binding\n        cover: string\n\
         store ^books(id: int): Book\n\n\
         fn cover(): string\n    return ^books?.binding?.cover\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_keyed_leaf_read_feeds_type_checks() {
    // `^books(1).tags(2)` is `string` (the layer's leaf type), but `f` returns `int`.
    let found = check_module(
        "saved-leaf",
        "module m\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(): int\n    return ^books(1).tags(2)\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn correctly_typed_group_and_leaf_reads_are_not_flagged() {
    // The group field and the keyed leaf both match their declared `string` use.
    let found = check_module(
        "saved-layer-ok",
        "module m\n\
         resource Book\n\
         \x20   tags(pos: int): string\n\
         \x20   versions(v: int)\n        title: string\n\
         store ^books(id: int): Book\n\n\
         fn title(): string\n    return ^books(1).versions(2).title\n\n\
         fn tag(): string\n    return ^books(1).tags(2)\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}
