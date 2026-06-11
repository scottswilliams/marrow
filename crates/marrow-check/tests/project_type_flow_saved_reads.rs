mod support;

use support::{assert_clean, check_module, check_module_report, with_code};

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
