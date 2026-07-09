use crate::support;
use marrow_check::{CallArgumentFault, DiagnosticPayload, MarrowType, check_project};
use marrow_schema::ScalarType;

use support::{
    check_module, check_module_program, check_module_report, config, resource_id, temp_project,
    with_code, write,
};

#[test]
fn rejects_a_wrong_argument_count_in_a_qualified_cross_module_call() {
    // `a::helper` takes one parameter; the qualified call in module `b` passes two.
    let root = temp_project("call-qualified", |root| {
        write(
            root,
            "src/a.mw",
            "module a\npub fn helper(x: int)\n    return\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\nfn caller()\n    a::helper(1, 2)\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_eq!(
        with_code(&report, "check.call_argument").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn rejects_a_positional_argument_of_the_wrong_type() {
    // `add` expects two ints; `true` is a bool.
    let found = check_module(
        "call-argtype",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(true, 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_named_argument_of_the_wrong_type() {
    // The named `a: true` passes a bool where `a` is an int.
    let found = check_module(
        "call-named-argtype",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(a: true, b: 2)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unresolved_argument_into_a_typed_parameter_is_flagged() {
    // Strict typing: `mystery` is unbound (unknown type), but `add`'s parameter is
    // `int`, so the argument is a `check.untyped_value` error — convert it first.
    // It is not a `check.call_argument` mismatch.
    let report = check_module_report(
        "call-argtype-unknown",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(mystery, 2)\n",
    );
    assert_eq!(
        with_code(&report, "check.untyped_value").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_call_return_type_feeds_further_type_checks() {
    // `makeInt()` is typed `int`, so `makeInt() + true` is an int-plus-bool error.
    let found = check_module(
        "call-return-type",
        "module m\n\
         fn makeInt(): int\n    return 1\n\n\
         fn caller()\n    var x = makeInt() + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_resource_to_a_mismatched_resource_parameter_is_flagged() {
    // Resources are nominally typed: a `Book` argument to a `Shelf` parameter names
    // a different resource and is a real argument mismatch.
    let found = check_module(
        "resource-arg",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         resource Shelf\n    name: string\n\
         store ^shelves(id: int): Shelf\n\n\
         fn useShelf(s: Shelf): bool\n    return true\n\n\
         fn f()\n    var book: Book\n    var ok = useShelf(book)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_resource_to_a_matching_resource_parameter_is_not_flagged() {
    // A `Book` argument to a `Book` parameter is the same resource, so it checks
    // clean — nominal typing accepts the matching resource.
    let found = check_module(
        "resource-arg-ok",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn useBook(b: Book): bool\n    return true\n\n\
         fn f()\n    var book: Book\n    var ok = useBook(book)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_module_constant_is_in_scope_and_typed() {
    // A top-level `const` is in scope (bare) for the module's functions and carries
    // its annotated type, so `M` is `int` and storing it into a `string` mismatches.
    let found = check_module(
        "module-const",
        "module m\nconst M: int = 5\n\nfn f()\n    var x: string = M\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_module_constant_reference_is_not_unresolved() {
    // The bare constant reference resolves (it is in scope), so it is not flagged
    // as an untyped value when stored into a matching place.
    let found = check_module(
        "module-const-ok",
        "module m\nconst M: int = 5\n\nfn f()\n    var x: int = M\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}
#[test]
fn single_name_loop_over_a_sequence_binds_int_positions() {
    // A local sequence is a 1-based integer-keyed tree, so a single loop variable
    // binds its `int` position: assigning it into `int` is clean.
    let clean = check_module(
        "for-seq-position-clean",
        "module m\nfn f(s: string)\n    for pos in std::text::split(s, \",\")\n        var x: int = pos\n",
        "check.assignment_type",
    );
    assert!(clean.is_empty(), "{clean:#?}");

    // Assigning the `int` position into a `string` place is a real mismatch.
    let mismatch = check_module(
        "for-seq-position-mismatch",
        "module m\nfn f(s: string)\n    for pos in std::text::split(s, \",\")\n        var x: string = pos\n",
        "check.assignment_type",
    );
    assert_eq!(mismatch.len(), 1, "{mismatch:#?}");
}

#[test]
fn an_unannotated_module_const_is_inferred_and_a_matching_use_is_not_flagged() {
    // `const M = 5` has an inferable `int` type; using it in `var x: int = M`
    // must not false-positive check.untyped_value.
    let found = check_module(
        "module-const-ok",
        "module m\nconst M = 5\nfn f()\n    var x: int = M\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unannotated_module_const_mismatch_is_caught() {
    // `const M = 5` is `int`; storing it into a `string` place is a real
    // mismatch.
    let found = check_module(
        "module-const-mismatch",
        "module m\nconst M = 5\nfn f()\n    var x: string = M\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_bare_saved_root_to_a_sequence_parameter_is_a_check_error() {
    // A saved store root is an in-place stream, not a local value: passing it to a
    // by-value `sequence[Id(^players)]` parameter would materialize the whole store.
    // It is a clean `check.call_argument` naming the by-value parameter, not a
    // deferred runtime fault.
    let found = check_module(
        "saved-root-to-sequence",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn take(xs: sequence[Id(^players)]): int\n    return count(xs)\n\n\
         fn f(): int\n    return take(^players)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::CallArgument(CallArgumentFault::SavedCollectionByValue {
            label: "take".into(),
            parameter: MarrowType::Sequence(Box::new(MarrowType::Identity("players".into()))),
        }),
        "{found:#?}"
    );
    // The error points at the `^players` argument, not the whole call head.
    assert_eq!(found[0].span.line, 10, "{found:#?}");
    assert_eq!(found[0].span.column, 17, "{found:#?}");
}

#[test]
fn passing_a_bare_saved_root_to_a_keyed_tree_parameter_is_a_check_error() {
    // The same rejection covers a keyed-tree parameter `scores(id: int): Player`: a
    // saved root cannot fill a by-value keyed map.
    let (found, program) = check_module_program(
        "saved-root-to-keyed-tree",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn take(scores(id: int): Player): int\n    return count(scores)\n\n\
         fn f(): int\n    return take(^players)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::CallArgument(CallArgumentFault::SavedCollectionByValue {
            label: "take".into(),
            parameter: MarrowType::LocalTree {
                keys: vec![MarrowType::Primitive(ScalarType::Int)],
                value: Box::new(MarrowType::Resource(resource_id(&program, "m", "Player"))),
            },
        }),
        "{found:#?}"
    );
}

#[test]
fn passing_a_saved_index_branch_to_a_sequence_parameter_is_a_check_error() {
    // An index branch streams identities in place; passing it to a by-value
    // `sequence[Id(^players)]` parameter is the same saved-collection rejection a
    // store root gets — it is not a local value to copy.
    let found = check_module(
        "saved-index-branch-to-sequence",
        "module m\n\
         resource Player\n    name: string\n    shelf: string\n\
         store ^players(id: int): Player\n    index byShelf(shelf, id)\n\n\
         fn take(xs: sequence[Id(^players)]): int\n    return count(xs)\n\n\
         fn f(): int\n    return take(^players.byShelf(\"a\"))\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::CallArgument(CallArgumentFault::SavedCollectionByValue {
            label: "take".into(),
            parameter: MarrowType::Sequence(Box::new(MarrowType::Identity("players".into()))),
        }),
        "{found:#?}"
    );
}

#[test]
fn passing_a_saved_keyed_sub_layer_to_a_sequence_parameter_is_a_check_error() {
    // A saved keyed sub-layer in value position is rejected before the call-argument
    // rule by the more precise `check.layer_not_value`: a partially keyed layer is an
    // in-place stream, never a value. The saved sub-collection is still un-passable by
    // value; only the diagnostic owner differs from the store-root case.
    let found = check_module(
        "saved-sublayer-to-sequence",
        "module m\n\
         resource Team\n    name: string\n    scores(player: string): int\n\
         store ^teams(id: int): Team\n\n\
         fn take(xs: sequence[string]): int\n    return count(xs)\n\n\
         fn f(id: Id(^teams)): int\n    return take(^teams(id).scores)\n",
        "check.layer_not_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_a_local_sequence_to_a_sequence_parameter_is_not_flagged() {
    // A local sequence value is a legitimate by-value argument: it checks clean.
    let found = check_module(
        "local-sequence-arg-ok",
        "module m\n\
         fn take(xs: sequence[string]): int\n    return count(xs)\n\n\
         fn f(): int\n    var xs = std::text::split(\"a,b\", \",\")\n    return take(xs)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn passing_a_local_keyed_map_to_a_keyed_tree_parameter_is_not_flagged() {
    // A local keyed map is a legitimate by-value argument to a keyed-tree parameter.
    let found = check_module(
        "local-keyed-map-arg-ok",
        "module m\n\
         fn take(scores(player: string): int): int\n    return count(scores)\n\n\
         fn f(): int\n    var scores(player: string): int\n    scores(\"a\") = 1\n    return take(scores)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn passing_a_keyed_tree_parameter_through_to_another_keyed_tree_parameter_is_not_flagged() {
    // A keyed-tree *parameter* is a caller-local value, so forwarding it to another
    // keyed-tree parameter is a clean by-value pass — not a saved collection.
    let found = check_module(
        "keyed-param-forward-ok",
        "module m\n\
         fn inner(scores(player: string): int): int\n    return count(scores)\n\n\
         fn outer(scores(player: string): int): int\n    return inner(scores)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn passing_a_saved_scalar_leaf_to_a_scalar_parameter_is_not_flagged() {
    // A saved scalar read is a single value, not a saved collection, so once its
    // maybe-presence is resolved it stays a valid by-value argument.
    let found = check_module(
        "saved-scalar-arg-ok",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn take(name: string): int\n    return 0\n\n\
         fn f(id: Id(^players)): int\n    return take(^players(id).name ?? \"\")\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn passing_a_bare_saved_root_to_a_sequence_std_helper_is_a_check_error() {
    // The shared call-argument owner closes the std-helper hole too: a saved string
    // collection cannot fill `text::join`'s by-value `sequence[string]` parameter.
    let found = check_module(
        "saved-root-to-std-sequence",
        "module m\n\
         resource Tag\n    name: string\n\
         store ^tags(name: string): Tag\n\n\
         fn f(): string\n    return std::text::join(^tags, \",\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn binding_a_saved_collection_to_a_var_is_a_check_error() {
    // A saved store root streams in place and has no local materialization, so
    // `var x = ^players` would materialize the un-materializable. Reject the binding
    // at its source so no laundered value can later fault clean-then-runtime.
    let found = check_module(
        "bind-saved-collection-var",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    var x = ^players\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    // The error points at the saved-collection value, not the whole statement.
    assert_eq!(found[0].span.line, 7, "{found:#?}");
    assert_eq!(found[0].span.column, 13, "{found:#?}");
}

#[test]
fn binding_a_saved_collection_to_a_const_is_a_check_error() {
    let found = check_module(
        "bind-saved-collection-const",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    const x = ^players\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn binding_a_saved_index_branch_to_a_var_is_a_check_error() {
    // An index branch streams identities in place; binding it to a local is the same
    // un-materializable saved collection a store root is.
    let found = check_module(
        "bind-saved-index-branch-var",
        "module m\n\
         resource Player\n    name: string\n    shelf: string\n\
         store ^players(id: int): Player\n    index byShelf(shelf, id)\n\n\
         fn f(): int\n    var x = ^players.byShelf(\"a\")\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn binding_a_saved_scalar_leaf_to_a_var_is_not_flagged() {
    // A saved scalar read is a single stored value, not a collection, so binding it
    // to a local is a legitimate value copy.
    let found = check_module(
        "bind-saved-scalar-var-ok",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(id: Id(^players)): string\n    var x = ^players(id).name\n    return x\n",
        "check.collection_unsupported",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn laundering_a_saved_collection_through_a_var_into_a_by_value_param_is_caught() {
    // The laundered variant of the by-value rejection: binding the saved collection to
    // a local first must not let it reach a by-value parameter unchecked. The binding
    // itself is the single root cause, so the call site needs no second diagnostic.
    let found = check_module(
        "launder-saved-collection-var-take",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn take(xs: sequence[Id(^players)]): int\n    return count(xs)\n\n\
         fn f(): int\n    var x = ^players\n    return take(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn laundering_a_saved_collection_through_a_var_into_a_std_helper_is_caught() {
    let found = check_module(
        "launder-saved-collection-var-join",
        "module m\n\
         resource Tag\n    name: string\n\
         store ^tags(name: string): Tag\n\n\
         fn f(): string\n    var x = ^tags\n    return std::text::join(x, \",\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn assigning_a_saved_collection_to_a_local_collection_var_is_a_check_error() {
    // The annotated sibling of the binding case: declaring `var x: sequence[...]` then
    // assigning a saved collection to it laundres the same un-materializable stream into
    // a local value. Reject the assignment so it cannot fault clean-then-runtime.
    let found = check_module(
        "assign-saved-collection-to-local",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    var x: sequence[Id(^players)]\n    x = ^players\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn whole_root_replacement_from_a_saved_root_is_not_flagged() {
    // Assigning a saved root to another saved root is a whole-root replacement, a
    // saved-to-saved write — not a local materialization — so it stays legal.
    let found = check_module(
        "whole-root-replacement-ok",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\
         store ^others(id: int): Player\n\n\
         fn f()\n    ^players = ^others\n",
        "check.collection_unsupported",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn returning_a_saved_root_as_a_sequence_is_a_check_error() {
    // The return boundary is the third laundering site: returning a bare saved root as
    // a declared `sequence[...]` would hand a caller an un-materializable stream that
    // checks clean and faults at runtime. The return materializes the un-materializable
    // exactly as a binding does, so it shares the one rejection.
    let found = check_module(
        "return-saved-root-as-sequence",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn give(): sequence[Id(^players)]\n    return ^players\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    // The error points at the returned value, not the whole statement.
    assert_eq!(found[0].span.line, 7, "{found:#?}");
    assert_eq!(found[0].span.column, 12, "{found:#?}");
}

#[test]
fn returning_a_saved_index_branch_as_a_sequence_is_a_check_error() {
    // An index branch streams identities in place; returning it as a declared
    // `sequence[...]` is the same un-materializable saved collection a store root is.
    let found = check_module(
        "return-saved-index-branch-as-sequence",
        "module m\n\
         resource Player\n    name: string\n    shelf: string\n\
         store ^players(id: int): Player\n    index byShelf(shelf, id)\n\n\
         fn give(): sequence[Id(^players)]\n    return ^players.byShelf(\"a\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn laundering_a_returned_saved_collection_through_its_caller_faults_nowhere() {
    // The full laundering path the lane exists to close: a caller that iterates the
    // returned saved collection must not reach the runtime. With the return rejected at
    // its source, the caller never receives the laundered value, so the program is a
    // clean check error rather than a clean-then-runtime fault.
    let found = check_module(
        "launder-returned-saved-collection",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn give(): sequence[Id(^players)]\n    return ^players\n\n\
         fn use_it(): int\n    return count(give())\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn returning_a_saved_scalar_leaf_is_not_flagged() {
    // A saved scalar read is a single stored value, not a collection, so returning it
    // by value is a legitimate value copy — the return rejection must not fire.
    let found = check_module(
        "return-saved-scalar-leaf-ok",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn give(id: Id(^players)): string\n    return ^players(id).name\n",
        "check.collection_unsupported",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn returning_a_local_sequence_is_not_flagged() {
    // A local sequence value is a legitimate by-value return: it checks clean.
    let found = check_module(
        "return-local-sequence-ok",
        "module m\n\
         fn give(): sequence[string]\n    return std::text::split(\"a,b\", \",\")\n",
        "check.collection_unsupported",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn binding_keys_of_a_saved_root_to_a_var_is_a_check_error() {
    // `keys(^players)` is a saved stream the runtime refuses to materialize as a value;
    // binding it to a local launders that stream exactly as the bare root does and
    // faulted at runtime through the bound local. It is a clean check error.
    let found = check_module(
        "bind-keys-saved-root",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    var x = keys(^players)\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn binding_values_of_a_saved_root_to_a_var_is_a_check_error() {
    // `values(^players)` is the same un-materializable saved stream as `keys(...)`.
    let found = check_module(
        "bind-values-saved-root",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    var x = values(^players)\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn binding_keys_of_a_saved_index_branch_to_a_var_is_a_check_error() {
    // An index branch wrapped by `keys(...)` is the same un-materializable saved
    // stream a store root is.
    let found = check_module(
        "bind-keys-index-branch",
        "module m\n\
         resource Player\n    name: string\n    shelf: string\n\
         store ^players(id: int): Player\n    index byShelf(shelf, id)\n\n\
         fn f(): int\n    var x = keys(^players.byShelf(\"a\"))\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn assigning_keys_of_a_saved_root_to_a_local_collection_var_is_a_check_error() {
    // Assigning `keys(^players)` to a local sequence target launders the same
    // un-materializable stream the binding case does.
    let found = check_module(
        "assign-keys-saved-root",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    var x: sequence[Id(^players)]\n    x = keys(^players)\n    return count(x)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn returning_keys_of_a_saved_root_as_a_sequence_is_a_check_error() {
    // Returning `keys(^players)` into a `sequence[...]` slot hands every caller the
    // same un-materializable stream the bare root does; reject it at this boundary.
    let found = check_module(
        "return-keys-saved-root",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn give(): sequence[Id(^players)]\n    return keys(^players)\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_values_of_a_saved_root_to_a_sequence_parameter_is_a_check_error() {
    // `values(^players)` is rejected at the `values` call — a saved root is iterated in
    // place, never materialized into a local sequence to copy by value.
    let found = check_module(
        "pass-values-saved-root",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn take(xs: sequence[Player]): int\n    return count(xs)\n\n\
         fn f(): int\n    return take(values(^players))\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn passing_keys_of_a_saved_root_to_a_sequence_std_helper_is_a_check_error() {
    // `keys` materializes a local collection; over a saved root it is rejected at the
    // `keys` call itself — saved data is iterated in place with `for ... in`.
    let found = check_module(
        "pass-keys-std-helper",
        "module m\n\
         resource Tag\n    label: string\n\
         store ^tags(name: string): Tag\n\n\
         fn f(): string\n    return std::text::join(keys(^tags), \",\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn iterating_keys_of_a_saved_root_in_a_for_loop_is_not_flagged() {
    // The documented streaming use — `for id in keys(^players)` — iterates the saved
    // stream directly and must stay legal: the by-value rejection must not over-reach.
    let report = check_module_report(
        "for-keys-saved-root",
        "module m\n\
         resource Player\n    name: string\n\
         store ^players(id: int): Player\n\n\
         fn f(): int\n    var total = 0\n    for id in ^players\n        total = total + 1\n    return total\n",
    );
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}
