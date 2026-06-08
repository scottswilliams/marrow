mod support;

use marrow_check::check_project;

use support::{check_module, config, temp_project, with_code, write};

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
    let found = check_module(
        "call-argtype-unknown",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(mystery, 2)\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    let mismatch = check_module(
        "call-argtype-unknown",
        "module m\n\
         fn add(a: int, b: int): int\n    return a\n\n\
         fn caller()\n    var x = add(mystery, 2)\n",
        "check.call_argument",
    );
    assert!(mismatch.is_empty(), "{mismatch:#?}");
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
         resource Book at ^books(id: int)\n    title: string\n\n\
         resource Shelf at ^shelves(id: int)\n    name: string\n\n\
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
         resource Book at ^books(id: int)\n    title: string\n\n\
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
fn a_for_binding_over_a_sequence_types_the_element() {
    // `std::text::split` yields `sequence[string]`, so `part` is `string` and
    // `part + 1` is string-plus-int.
    let found = check_module(
        "for-elem",
        "module m\nfn f(s: string)\n    for part in std::text::split(s, \",\")\n        var x = part + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
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
