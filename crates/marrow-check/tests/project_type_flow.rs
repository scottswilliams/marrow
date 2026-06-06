mod support;

use marrow_check::{
    AppendTargetDiagnostic, ConversionTarget, ConversionUnsupportedSourceDiagnostic,
    DiagnosticPayload, MarrowType, ScalarType, check_project,
};

use support::{
    assert_clean, check_module, check_module_report, config, temp_project, with_code, write,
};

fn conversion_source_payload(target: ConversionTarget, source: MarrowType) -> DiagnosticPayload {
    DiagnosticPayload::ConversionUnsupportedSource(ConversionUnsupportedSourceDiagnostic {
        target,
        source,
        accepted_sources: target.accepted_source_types(),
    })
}

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
fn a_nested_group_field_read_resolves_its_type() {
    // A read through nested group layers resolves to the innermost field's type,
    // so a typed return of it is not flagged as an untyped value.
    let found = check_module(
        "nested-read",
        "module m\n\
         resource Book at ^books(id: int)\n    required title: string\n    \
         versions(version: int)\n        required title: string\n        \
         comments(pos: int)\n            required text: string\n\n\
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
         resource Book at ^books(id: int)\n    required title: string\n    \
         versions(version: int)\n        required title: string\n        \
         comments(pos: int)\n            required text: string\n\n\
         fn f()\n    const n: int = ^books(1).versions(2).comments(3).text\n",
        "check.assignment_type",
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
fn a_saved_field_read_feeds_the_return_type_check() {
    // `^books(1).title` is `string` from the schema, but `f` returns `int`.
    let found = check_module(
        "saved-field-return",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
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
         resource Book at ^books(id: int)\n    currentVersion: int\n\n\
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
         resource Book at ^books(id: int)\n    title: string\n\n\
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
         resource Book at ^books(id: int)\n    title: string\n\n\
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
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f(): string\n    var book: Book\n    return book.title\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
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
fn a_whole_resource_read_into_a_local_types_its_fields() {
    // `^books(1)` reads the whole record as a `Book`; `b.title` then resolves to
    // `string` from the schema, so `+ 1` is string-plus-int.
    let found = check_module(
        "whole-read-field",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
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
fn exists_and_append_builtin_return_types_feed_checks() {
    // `exists` returns `bool` and `append` returns `int`; using them in mismatched
    // operators is caught.
    let found = check_module(
        "builtin-returns",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n    tags(pos: int): string\n\n\
         fn f()\n    var a = exists(^books(1)) + 1\n    var b = append(^books(1).tags, \"t\") and true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn append_to_a_group_layer_is_a_check_error() {
    let found = check_module(
        "append-group-layer",
        "module m\n\
         resource Log at ^log(name: string)\n    items(pos: int)\n        required n: int\n\n\
         fn add(name: string): int\n    return append(^log(name).items, 1)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::AppendTarget(AppendTargetDiagnostic::GroupLayer),
        "{found:#?}"
    );
}

#[test]
fn append_to_a_keyed_leaf_layer_still_checks_clean() {
    let report = check_module_report(
        "append-leaf-layer",
        "module m\n\
         resource Log at ^log(name: string)\n    items(pos: int): int\n\n\
         fn add(name: string): int\n    return append(^log(name).items, 1)\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn coalesce_yields_the_default_type() {
    // `path ?? default` types to the path's leaf-or-default type; with a string
    // default it is `string`, so `+ 1` is string-plus-int.
    let found = check_module(
        "coalesce-return",
        "module m\n\
         resource Book at ^books(id: int)\n    title: string\n\n\
         fn f()\n    var x = (^books(1).title ?? \"none\") + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn coalesce_rejects_a_present_non_path_left_operand() {
    // `??` only defaults an absent read; a literal (or any always-present value)
    // on the left has nothing to default, so it is an operator misuse.
    let found = check_module(
        "coalesce-non-path",
        "module m\nfn f()\n    var x = 1 ?? 2\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn coalesce_rejects_a_mismatched_default_type() {
    // The default must match the path's leaf type: an `int` field defaulted with a
    // string is an operator misuse.
    let found = check_module(
        "coalesce-mismatch",
        "module m\n\
         resource Book at ^books(id: int)\n    pages: int\n\n\
         fn f()\n    var x = ^books(1).pages ?? \"none\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_std_call_return_type_feeds_operator_checks() {
    // `std::text::length` returns `int`, so `+ true` is int-plus-bool.
    let found = check_module(
        "std-return-op",
        "module m\nfn f()\n    var x = std::text::length(\"hi\") + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_std_call_return_type_feeds_the_return_type_check() {
    // `std::clock::now()` is `instant`, but `f` returns `int`.
    let found = check_module(
        "std-return-mismatch",
        "module m\nfn f(): int\n    return std::clock::now()\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_correctly_typed_std_call_return_is_not_flagged() {
    // `std::text::length` returns `int`, matching `f`'s declared `int` return.
    let found = check_module(
        "std-return-ok",
        "module m\nfn f(): int\n    return std::text::length(\"hi\")\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_sequence_returning_std_call_against_a_scalar_return_is_flagged() {
    // `std::text::split` returns `sequence[string]`; returning it from an `int`
    // function is a real type mismatch — a sequence is not a scalar.
    let found = check_module(
        "std-return-seq",
        "module m\nfn f(): int\n    return std::text::split(\"a,b\", \",\")\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_sequence_returning_std_call_against_a_matching_return_is_not_flagged() {
    // Returning `sequence[string]` from a `sequence[string]` function recurses into
    // the element type and checks clean.
    let found = check_module(
        "std-return-seq-ok",
        "module m\nfn f(): sequence[string]\n    return std::text::split(\"a,b\", \",\")\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_conversion_call_return_type_feeds_operator_checks() {
    // `int(raw)` returns `int`, so `+ true` is int-plus-bool.
    let found = check_module(
        "conv-return-op",
        "module m\nfn f(raw: unknown)\n    var x = int(raw) + true\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_conversion_into_a_mismatched_annotated_place_is_flagged() {
    // `int(raw)` is `int`, but the place is `string`.
    let found = check_module(
        "conv-assign-bad",
        "module m\nfn f(raw: unknown)\n    const s: string = int(raw)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_conversion_into_a_matching_annotated_place_is_not_flagged() {
    // `int(raw)` is `int`, matching the declared `int` place — the documented
    // `const n: int = int(raw)` pattern checks clean.
    let found = check_module(
        "conv-assign-ok",
        "module m\nfn f(raw: unknown)\n    const n: int = int(raw)\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn bytes_conversion_rejects_a_known_non_string_source() {
    let found = check_module(
        "bytes-conv-int",
        "module m\nfn f(): bytes\n    return bytes(int(9))\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        conversion_source_payload(
            ConversionTarget::Bytes,
            MarrowType::Primitive(ScalarType::Int)
        ),
        "{found:#?}"
    );
}

#[test]
fn bytes_conversion_accepts_string_bytes_and_unknown_sources() {
    let report = check_module_report(
        "bytes-conv-ok",
        "module m\n\
         fn fromString(s: string): bytes\n    return bytes(s)\n\n\
         fn fromBytes(b: bytes): bytes\n    return bytes(b)\n\n\
         fn fromUnknown(raw: unknown): bytes\n    return bytes(raw)\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn conversion_calls_reject_known_unsupported_sources() {
    let found = check_module(
        "conv-known-bad-sources",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn dateFromInt(): date\n    return date(1)\n\n\
         fn durationFromInt(): duration\n    return duration(1)\n\n\
         fn boolFromString(): bool\n    return bool(\"true\")\n\n\
         fn decimalFromBool(): decimal\n    return decimal(true)\n\n\
         fn enumToInt(): int\n    return int(Color::green)\n\n\
         fn enumToString(): string\n    return string(Color::green)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 6, "{found:#?}");
    let color = MarrowType::Enum {
        module: "m".into(),
        name: "Color".into(),
    };
    assert_eq!(
        found
            .iter()
            .map(|diagnostic| diagnostic.payload.clone())
            .collect::<Vec<_>>(),
        vec![
            conversion_source_payload(
                ConversionTarget::Date,
                MarrowType::Primitive(ScalarType::Int)
            ),
            conversion_source_payload(
                ConversionTarget::Duration,
                MarrowType::Primitive(ScalarType::Int)
            ),
            conversion_source_payload(
                ConversionTarget::Bool,
                MarrowType::Primitive(ScalarType::Str)
            ),
            conversion_source_payload(
                ConversionTarget::Decimal,
                MarrowType::Primitive(ScalarType::Bool)
            ),
            conversion_source_payload(ConversionTarget::Int, color.clone()),
            conversion_source_payload(ConversionTarget::Str, color),
        ],
        "{found:#?}"
    );
}

#[test]
fn interpolation_rejects_enum_values() {
    let found = check_module(
        "interp-enum",
        "module m\n\
         enum Color\n    red\n    green\n\n\
         fn f(c: Color): string\n    return $\"c={c}\"\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::InterpolationUnsupportedSource {
            source: MarrowType::Enum {
                module: "m".into(),
                name: "Color".into(),
            },
        }
    );
}

#[test]
fn an_error_code_conversion_into_an_error_code_place_is_not_flagged() {
    // `ErrorCode(raw)` is `ErrorCode`, matching the declared `ErrorCode` place —
    // the documented `const code: ErrorCode = ErrorCode(raw)` conversion checks
    // clean (no false `check.untyped_value`).
    let found = check_module(
        "conv-error-code",
        "module m\nfn f(raw: unknown)\n    const code: ErrorCode = ErrorCode(raw)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn type_surface_count_builtin_result_is_an_int() {
    let report = check_module_report(
        "count-result-int",
        "module m\n\
         resource Book at ^books(id: int)\n    tags(pos: int): string\n\n\
         fn countBooks(): int\n    return count(^books)\n\n\
         fn countTags(id: Id(^books)): int\n    return count(^books(id).tags)\n",
    );
    assert_clean(&report);
}

#[test]
fn type_surface_count_of_a_non_path_is_not_an_int() {
    let found = check_module(
        "count-non-path",
        "module m\nfn f(): int\n    return count(1)\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn type_surface_caught_error_fields_have_declared_types() {
    let report = check_module_report(
        "caught-error-fields",
        "module m\n\
         fn f()\n\
         \x20   try\n        throw Error(code: \"x.y\", message: \"boom\")\n\
         \x20   catch err: Error\n\
         \x20       const code: ErrorCode = err.code\n\
         \x20       const message: string = err.message\n",
    );
    assert_clean(&report);
}

#[test]
fn type_surface_ledger_reads_and_traversals_have_concrete_types() {
    let report = check_module_report(
        "ledger-type-surfaces",
        "module m\n\
         resource Account at ^accounts(code: string)\n    required name: string\n    amounts(pos: int): decimal\n\n\
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
         resource Book at ^books(id: int)\n    versions(v: int)\n        title: string\n\n\
         fn f(): int\n    return ^books(1).versions(2).title\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_singleton_field_read_feeds_type_checks() {
    // `^settings.theme` on a keyless singleton resource (`Settings at ^settings`)
    // is `string` from the schema, not Unknown — so a typed use never
    // false-positives check.untyped_value, and a real mismatch (returning it
    // from an `int` function) is caught.
    let found = check_module(
        "singleton-field",
        "module m\n\
         resource Settings at ^settings\n    theme: string\n\n\
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
         resource Settings at ^settings\n    theme: string\n\n\
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
         resource Settings at ^settings\n    counts(name: string): int\n\n\
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
         resource Settings at ^settings\n    tokens(pos: int)\n        kind: string\n\n\
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
         resource Settings at ^settings\n    theme: string\n    required maxLoans: int\n\n\
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
         resource Patient at ^patients(id: int)\n\
         \x20   name\n        first: string\n        last: string\n\n\
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
         resource Patient at ^patients(id: int)\n\
         \x20   name\n        first: string\n        last: string\n\n\
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
         resource Book at ^books(id: int)\n\
         \x20   binding\n        cover: string\n\n\
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
         resource Book at ^books(id: int)\n\
         \x20   binding\n        cover: string\n\n\
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
         resource Book at ^books(id: int)\n    tags(pos: int): string\n\n\
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
         resource Book at ^books(id: int)\n\
         \x20   tags(pos: int): string\n\
         \x20   versions(v: int)\n        title: string\n\n\
         fn title(): string\n    return ^books(1).versions(2).title\n\n\
         fn tag(): string\n    return ^books(1).tags(2)\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
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
