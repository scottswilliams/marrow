use crate::support;
use crate::support_conversion;
use marrow_check::{ConversionTarget, DiagnosticPayload, MarrowType, ScalarType, check_project};

use support::{
    assert_clean, check_module, check_module_report, check_script, config, temp_project, with_code,
    write,
};
use support_conversion::conversion_source_payload;

#[test]
fn a_conversion_rejects_an_unsupported_source_and_lists_the_accepted_sources() {
    let found = check_module(
        "convert-bool-from-string",
        "module m\n\
         fn caller(s: string): bool\n    return bool(s)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        conversion_source_payload(
            ConversionTarget::Bool,
            MarrowType::Primitive(ScalarType::Str)
        ),
        "{found:#?}"
    );
}

#[test]
fn the_string_and_error_code_conversions_have_distinct_source_sets() {
    // `string` and `ErrorCode` both store as a string scalar, so the source spelling
    // — not the scalar — is the conversion's identity. `string(...)` accepts every
    // scalar; `ErrorCode(...)` accepts only a string source. A `bytes` argument is
    // therefore clean for `string` but a mismatch for `ErrorCode`.
    let clean = check_module(
        "convert-string-from-bytes",
        "module m\n\
         fn caller(b: bytes): string\n    return string(b)\n",
        "check.call_argument",
    );
    assert!(clean.is_empty(), "{clean:#?}");

    let rejected = check_module(
        "convert-error-code-from-bytes",
        "module m\n\
         fn caller(b: bytes): ErrorCode\n    return ErrorCode(b)\n",
        "check.call_argument",
    );
    assert_eq!(rejected.len(), 1, "{rejected:#?}");
    assert_eq!(
        rejected[0].payload,
        conversion_source_payload(
            ConversionTarget::ErrorCode,
            MarrowType::Primitive(ScalarType::Bytes)
        ),
        "{rejected:#?}"
    );
}

#[test]
fn a_builtin_call_is_not_arity_checked_and_an_unknown_call_is_not_a_mismatch() {
    // `print` is a builtin (dispatched before user functions) and `mystery` does
    // not resolve to a declared function; neither is an arity/argument mismatch.
    let found = check_module(
        "call-skip",
        "module m\n\
         fn caller()\n    print(1, 2, 3)\n    var x = mystery(1, 2)\n",
        "check.call_argument",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_call_to_an_undefined_function_is_flagged() {
    // Strict typing, runtime parity (run.unknown_function): a call to a name that
    // is neither a builtin nor a declared function is an unresolved call.
    let found = check_module(
        "call-unknown",
        "module m\n\
         fn caller()\n    mystery(1, 2)\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_call_to_an_unknown_std_submodule_is_flagged() {
    // `std::bogus::foo()` names no real std module (the std-module set derived
    // from the shared stdlib table), so it is not a builtin — it is reported
    // consistently with `use std::bogus` rejection, rather than silently
    // type-checking.
    let found = check_module(
        "call-std-bogus",
        "module m\n\
         fn caller()\n    std::bogus::foo()\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_call_to_a_known_std_submodule_is_not_flagged() {
    // A real std submodule call stays a builtin and is not unresolved.
    let found = check_module(
        "call-std-known",
        "module m\n\
         fn caller()\n    var n = std::text::length(\"hi\")\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_builtin_call_is_not_an_unresolved_call() {
    // Builtins dispatch before user functions, so they never resolve to a program
    // function — but they are defined, not unresolved.
    let found = check_module(
        "call-builtin",
        "module m\n\
         fn caller()\n    print(1, 2, 3)\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_call_to_a_defined_function_is_not_an_unresolved_call() {
    let found = check_module(
        "call-defined",
        "module m\n\
         fn helper(): int\n    return 1\n\n\
         fn caller()\n    var x = helper()\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_resource_constructor_is_not_an_unresolved_call() {
    // `Book(...)` constructs a resource value; it is a known
    // declared resource, not an undefined function.
    let found = check_module(
        "ctor-resource",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn caller()\n    var b = Book(title: \"a\")\n",
        "check.unresolved_call",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_resource_constructor_checks_field_arguments() {
    let found = check_module(
        "ctor-field-type",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn caller()\n    var b = Book(title: 1, shelf: \"fiction\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn resource_constructor_fields_resolve_resource_types_by_declaring_module() {
    let report = check_module_report(
        "ctor-resource-field-owner",
        "module m\n\
         resource Address\n    city: string\n\n\
         resource Person\n    required name: string\n    address: Address\n\n\
         fn caller()\n    var p = Person(name: \"Sam\", address: Address(city: \"Paris\"))\n",
    );
    assert!(
        with_code(&report, "check.call_argument").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn same_module_resource_annotation_beats_foreign_enum_fallback() {
    let root = temp_project("resource-type-before-foreign-enum", |root| {
        write(root, "src/a.mw", "module a\npub enum Address\n    ok\n");
        write(
            root,
            "src/m.mw",
            "module m\n\
             resource Address\n    city: string\n\n\
             fn make(): Address\n    return Address(city: \"Paris\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        with_code(&report, "check.return_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn user_maybe_return_call_must_be_resolved_at_the_caller() {
    let report = check_module_report(
        "user-maybe-return-call-sites",
        "module m\n\
         fn find(): maybe int\n\
         \x20   return absent\n\n\
         fn unresolved(): int\n\
         \x20   return find()\n\n\
         fn coalesced(): int\n\
         \x20   return find() ?? -1\n\n\
         fn guarded(): int\n\
         \x20   if const n = find()\n\
         \x20       return n\n\
         \x20   return -1\n\n\
         fn has_value(): bool\n\
         \x20   return exists(find())\n",
    );

    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn user_maybe_return_propagates_only_through_maybe_return_sites() {
    let report = check_module_report(
        "user-maybe-return-propagation",
        "module m\n\
         fn b(): maybe int\n\
         \x20   return absent\n\n\
         fn a(): maybe int\n\
         \x20   return b()\n\n\
         fn unresolved(): int\n\
         \x20   return a()\n\n\
         fn resolved(): int\n\
         \x20   return a() ?? -1\n",
    );

    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn exists_does_not_narrow_a_later_maybe_function_call() {
    let report = check_module_report(
        "user-maybe-return-exists-call-boundary",
        "module m\n\
         fn find(): maybe int\n\
         \x20   return absent\n\n\
         fn caller(): int\n\
         \x20   if exists(find())\n\
         \x20       return find()\n\
         \x20   return -1\n",
    );

    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn assert_absent_does_not_resolve_a_maybe_function_call() {
    let report = check_module_report(
        "user-maybe-return-assert-absent-boundary",
        "module m\n\
         resource Book\n\
         \x20   title: string\n\
         store ^books(id: int): Book\n\n\
         fn missing(): maybe int\n\
         \x20   return absent\n\n\
         fn caller()\n\
         \x20   std::assert::absent(missing())\n\
         \x20   std::assert::absent(next(^books))\n",
    );

    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 2, "{:#?}", report.diagnostics);
}

#[test]
fn assert_absent_rejects_non_path_arguments() {
    let report = check_module_report(
        "assert-absent-path-shape",
        "module m\n\
         resource Book\n\
         \x20   title: string\n\
         store ^books(id: int): Book\n\n\
         fn always(): int\n\
         \x20   return 1\n\n\
         fn caller(id: int)\n\
         \x20   std::assert::absent(^books(id).title)\n\
         \x20   std::assert::absent(1)\n\
         \x20   std::assert::absent(id)\n\
         \x20   std::assert::absent(always())\n",
    );

    let found = with_code(&report, "check.call_argument");
    assert_eq!(found.len(), 3, "{:#?}", report.diagnostics);
}

#[test]
fn maybe_return_absence_forms_are_checked_against_the_signature() {
    let report = check_module_report(
        "user-maybe-return-absence-forms",
        "module m\n\
         fn falls_through(): maybe int\n\
         \x20   const n = 1\n\n\
         fn plain_return(): maybe int\n\
         \x20   return\n\n\
         fn absent_from_plain(): int\n\
         \x20   return absent\n\n\
         fn absent_from_void()\n\
         \x20   return absent\n",
    );

    assert_eq!(
        with_code(&report, "check.missing_return").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert_eq!(
        with_code(&report, "check.return_value").len(),
        3,
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn saved_maybe_read_can_be_returned_from_maybe_function() {
    let report = check_module_report(
        "user-maybe-return-saved-read",
        "module m\n\
         resource Book\n\
         \x20   subtitle: string\n\
         store ^books(id: int): Book\n\n\
         fn subtitle(id: int): maybe string\n\
         \x20   return ^books(id).subtitle\n\n\
         fn unresolved(id: int): string\n\
         \x20   return subtitle(id)\n\n\
         fn resolved(id: int): string\n\
         \x20   return subtitle(id) ?? \"missing\"\n",
    );

    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn same_module_resource_annotation_beats_private_foreign_enum_diagnostic() {
    let root = temp_project("resource-type-before-private-foreign-enum", |root| {
        write(root, "src/a.mw", "module a\nenum Address\n    ok\n");
        write(
            root,
            "src/m.mw",
            "module m\n\
             resource Address\n    city: string\n\n\
             fn make(): Address\n    return Address(city: \"Paris\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        with_code(&report, "check.private_enum").is_empty(),
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.return_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_resource_constructor_rejects_unknown_fields() {
    let found = check_module(
        "ctor-unknown-field",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn caller()\n    var b = Book(title: \"a\", pages: 3)\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_resource_constructor_rejects_duplicate_fields() {
    let found = check_module(
        "ctor-duplicate-field",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn caller()\n    var b = Book(title: \"a\", title: \"b\", shelf: \"fiction\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::DuplicateNamedArgument("title".into())
    );
}

#[test]
fn a_resource_constructor_requires_required_fields() {
    let found = check_module(
        "ctor-required-field",
        "module m\n\
         resource Book\n    required title: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n\
         fn caller()\n    var b = Book(shelf: \"fiction\")\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_qualified_resource_constructor_is_not_an_unresolved_call() {
    let root = temp_project("qualified-resource-constructor", |root| {
        write(
            root,
            "src/library.mw",
            "module library\nresource Book\n    title: string\n",
        );
        write(
            root,
            "src/app.mw",
            "module app\nuse library\nfn caller()\n    var b = library::Book(title: \"Mort\")\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn an_unknown_call_in_a_module_less_script_is_flagged() {
    // A module-less script joins the program under the empty module name, so its
    // own calls resolve against it: a call naming a function the script does not
    // declare is `check.unresolved_call`, not a silently-accepted reference.
    let found = check_script(
        "call-script",
        "fn f()\n    mystery()\n",
        "check.unresolved_call",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_primary_root_loop_binds_identities() {
    let report = check_module_report(
        "root-loop-identities",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn titles()\n    for id in ^books\n        var typed: Id(^books) = id\n",
    );
    assert_clean(&report);
}

#[test]
fn a_two_name_primary_root_loop_binds_identity_and_resource() {
    let report = check_module_report(
        "root-loop-entries",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn titles()\n    for id, book in ^books\n        var typed: Id(^books) = id\n        var title: string = book.title\n",
    );
    assert_clean(&report);
}

#[test]
fn a_sequence_layer_loop_binds_keys() {
    let report = check_module_report(
        "layer-loop-keys",
        "module m\n\
         resource Book\n    tags: sequence[string]\n\
         store ^books(id: int): Book\n\n\
         fn tags(id: Id(^books))\n    for pos in ^books(id).tags\n        var typed: int = pos\n",
    );
    assert_clean(&report);
}

#[test]
fn a_keyed_group_layer_loop_binds_group_entry_values() {
    let report = check_module_report(
        "group-layer-loop-elements",
        "module m\n\
         resource Book\n    versions(version: int)\n        required title: string\n\
         store ^books(id: int): Book\n\n\
         fn titles(id: Id(^books))\n    for version in ^books(id).versions\n        var typed: int = version\n    for n, version in ^books(id).versions\n        var typed: int = n\n        var title: string = version.title\n",
    );
    assert_clean(&report);
}

#[test]
fn a_typed_keyed_resource_layer_loop_binds_key_and_entry_value() {
    let report = check_module_report(
        "typed-keyed-resource-layer-loop",
        "module m\n\
         resource Comment\n\
         \x20   required body: string\n\
         \x20   meta\n\
         \x20       author: string\n\
         resource Post\n\
         \x20   comments(seq: int): Comment\n\
         store ^posts(id: int): Post\n\n\
         fn comments(id: Id(^posts))\n\
         \x20   for seq, comment in ^posts(id).comments\n\
         \x20       var typed_seq: int = seq\n\
         \x20       var body: string = comment.body\n\
         \x20       var author: string = comment.meta.author\n",
    );
    assert_clean(&report);
}

#[test]
fn single_name_entries_loops_are_rejected() {
    let found = check_module(
        "single-name-entries",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    for entry in entries(^books)\n        print($\"{entry}\")\n",
        "check.collection_unsupported",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn entries_calls_are_rejected_outside_two_name_loop_heads() {
    for (name, body) in [
        ("entries-const", "const rows = entries(^books)\n"),
        ("entries-return", "return entries(^books)\n"),
        (
            "entries-match",
            "match entries(^books)\n        missing\n            return\n",
        ),
        (
            "entries-local-const",
            "var scores(player: string): int\n    const rows = entries(scores)\n",
        ),
    ] {
        let found = check_module(
            name,
            &format!(
                "module m\n\
                 resource Book\n    required title: string\n\
                 store ^books(id: int): Book\n\n\
                 fn f()\n    {body}",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn entries_loop_heads_reject_nested_and_pass_through_wrappers() {
    for (name, body) in [
        (
            "local-nested-entries",
            "var scores(player: string): int\n    for player, score in entries(entries(scores))\n        print($\"{player}: {score}\")\n",
        ),
        (
            "saved-nested-entries",
            "for id, book in entries(entries(^books))\n        print($\"{id}: {book.title}\")\n",
        ),
        (
            "saved-reversed-nested-entries",
            "for id, book in reversed(entries(entries(^books)))\n        print($\"{id}: {book.title}\")\n",
        ),
        (
            "saved-entries-values",
            "for id, book in entries(values(^books))\n        print($\"{id}: {book.title}\")\n",
        ),
    ] {
        let found = check_module(
            name,
            &format!(
                "module m\n\
                 resource Book\n    required title: string\n\
                 store ^books(id: int): Book\n\n\
                 fn f()\n    {body}",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn entries_loop_heads_reject_non_collection_arguments() {
    for (name, iterable) in [
        ("entries-binary", "entries(x + 1)"),
        ("reversed-entries-binary", "reversed(entries(x + 1))"),
    ] {
        let found = check_module(
            name,
            &format!(
                "module m\n\
                 fn f()\n    var x = 1\n    for key, value in {iterable}\n        print($\"{{key}}: {{value}}\")\n",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn top_level_entries_const_reports_loop_head_only() {
    let root = temp_project("entries-const-value", |root| {
        write(
            root,
            "src/books.mw",
            "module books\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             const rows = entries(^books)\n",
        );
    });

    let (report, _program) = check_project(&root, &config()).expect("check");
    let found = with_code(&report, "check.collection_unsupported");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn local_keyed_tree_two_name_loops_bind_key_and_value() {
    let report = check_module_report(
        "local-tree-two-name-loop",
        "module m\n\
         fn f(): int\n    var scores(player: string): int\n    scores(\"bob\") = 7\n    var total = 0\n    for player, score in scores\n        const key_ok: string = player\n        total = total + score\n    return total\n",
    );
    assert_clean(&report);
}

#[test]
fn local_sequence_entries_loops_bind_position_and_value() {
    let report = check_module_report(
        "local-sequence-entries-loop",
        "module m\n\
         fn f()\n    var xs: sequence[int]\n    xs(1) = 1\n    xs(2) = 2\n    for pos, x in entries(xs)\n        const pos_ok: int = pos\n        const value_ok: int = x\n    for pos, x in reversed(entries(xs))\n        const pos_ok: int = pos\n        const value_ok: int = x\n",
    );
    assert_clean(&report);
}

#[test]
fn local_sequence_key_and_value_views_bind_positions_and_values() {
    let report = check_module_report(
        "local-sequence-key-value-views",
        "module m\n\
         fn f()\n    var xs: sequence[int]\n    xs(1) = 1\n    xs(2) = 2\n    for pos in keys(xs)\n        const pos_ok: int = pos\n    for x in values(xs)\n        const value_ok: int = x\n    for pos in reversed(keys(xs))\n        const reversed_pos_ok: int = pos\n    for x in reversed(values(xs))\n        const reversed_value_ok: int = x\n",
    );
    assert_clean(&report);
}

#[test]
fn two_name_local_key_and_value_views_are_rejected() {
    for (name, setup, iterable) in [
        (
            "sequence-keys",
            "var xs: sequence[int]\n    xs(1) = 1\n",
            "keys(xs)",
        ),
        (
            "sequence-values",
            "var xs: sequence[int]\n    xs(1) = 1\n",
            "values(xs)",
        ),
        (
            "sequence-reversed-keys",
            "var xs: sequence[int]\n    xs(1) = 1\n",
            "reversed(keys(xs))",
        ),
        (
            "sequence-reversed-values",
            "var xs: sequence[int]\n    xs(1) = 1\n",
            "reversed(values(xs))",
        ),
        (
            "tree-keys",
            "var scores(player: string): int\n    scores(\"p1\") = 10\n",
            "keys(scores)",
        ),
        (
            "tree-values",
            "var scores(player: string): int\n    scores(\"p1\") = 10\n",
            "values(scores)",
        ),
        (
            "tree-reversed-keys",
            "var scores(player: string): int\n    scores(\"p1\") = 10\n",
            "reversed(keys(scores))",
        ),
        (
            "tree-reversed-values",
            "var scores(player: string): int\n    scores(\"p1\") = 10\n",
            "reversed(values(scores))",
        ),
    ] {
        let found = check_module(
            &format!("two-name-local-{name}"),
            &format!(
                "module m\n\
                 fn f()\n    {setup}    for first, second in {iterable}\n        print($\"{{first}}={{second}}\")\n",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn two_name_keys_and_values_loops_do_not_bind_pair_types() {
    for wrapper in ["keys", "values"] {
        let found = check_module(
            &format!("two-name-{wrapper}"),
            &format!(
                "module m\n\
                 resource Book\n    required title: string\n\
                 store ^books(id: int): Book\n\n\
                 fn f()\n    for first, second in {wrapper}(^books)\n        var n = first + 1\n",
            ),
            "check.operator_type",
        );
        assert!(found.is_empty(), "{wrapper}: {found:#?}");
    }
}

#[test]
fn two_name_sequence_loops_are_rejected_even_when_bindings_are_unused() {
    for (name, iterable) in [("direct", "xs"), ("reversed", "reversed(xs)")] {
        let found = check_module(
            &format!("two-name-{name}-sequence-unused-bindings"),
            &format!(
                "module m\n\
                 fn f(): string\n    var xs: sequence[int]\n    xs(1) = 1\n    xs(2) = 2\n    var out: string = \"\"\n    for pos, x in {iterable}\n        out = $\"{{out}}row;\"\n    return out\n",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn a_unique_index_lookup_loop_binds_the_identity() {
    let report = check_module_report(
        "unique-index-loop",
        "module m\n\
         resource Book\n    isbn: string\n\
         store ^books(id: int): Book\n\n    index byIsbn(isbn) unique\n\n\
         fn f(isbn: string)\n    for id in ^books.byIsbn(isbn)\n        var typed: Id(^books) = id\n",
    );
    assert_clean(&report);
}

#[test]
fn unique_index_lookup_arguments_are_checked() {
    let found = check_module(
        "unique-index-args",
        "module m\n\
         resource Book\n    isbn: string\n\
         store ^books(id: int): Book\n\n    index byIsbn(isbn) unique\n\n\
         fn f()\n    \
         const missing = ^books.byIsbn()\n    \
         const extra = ^books.byIsbn(\"978\", 1)\n    \
         const wrong = ^books.byIsbn(123)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 3, "{found:#?}");
}

#[test]
fn named_saved_root_key_arguments_are_rejected() {
    let found = check_module(
        "named-saved-root-key-args",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    var book = Book(title: \"x\")\n    ^books(id: 1) = book\n    const title = ^books(id: 1).title\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn named_saved_layer_key_arguments_are_rejected() {
    let found = check_module(
        "named-saved-layer-key-args",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    ^books(1).tags(pos: 1) = \"x\"\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn named_saved_index_key_arguments_are_rejected() {
    let found = check_module(
        "named-saved-index-key-args",
        "module m\n\
         resource Book\n    isbn: string\n\
         store ^books(id: int): Book\n\n    index byIsbn(isbn) unique\n\n\
         fn f()\n    const found = exists(^books.byIsbn(isbn: \"x\"))\n",
        "check.call_argument",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn partial_non_unique_index_branches_bind_the_store_identity() {
    // Every single-name loop over a non-unique index branch binds the store
    // identity it streams, whether the prefix is empty, partial, or the full
    // field prefix; it never binds an intermediate index field.
    let report = check_module_report(
        "partial-index-loop",
        "module m\n\
         resource Book\n    author: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n    index byAuthorShelf(author, shelf, id)\n\n\
         fn f()\n    \
         for id in ^books.byAuthorShelf\n        var bare_id: Id(^books) = id\n    \
         for id in ^books.byAuthorShelf(\"ann\")\n        var prefix_id: Id(^books) = id\n    \
         for id in ^books.byAuthorShelf(\"ann\", \"fiction\")\n        var typed_id: Id(^books) = id\n",
    );
    assert_clean(&report);
}

#[test]
fn trailing_range_index_argument_checks_clean() {
    let report = check_module_report(
        "trailing-index-range",
        "module m\n\
         resource Post\n    published: int\n    required title: string\n\
         store ^posts(id: int): Post\n\n    index byDate(published, id)\n\n\
         fn f(start: int, end: int)\n    for id in ^posts.byDate(start..end)\n        var typed_id: Id(^posts) = id\n    for post_id, post in ^posts.byDate(start..end)\n        var typed_post_id: Id(^posts) = post_id\n        var title: string = post.title\n",
    );
    assert_clean(&report);
}

#[test]
fn trailing_range_store_keyspace_argument_checks_clean() {
    let report = check_module_report(
        "trailing-root-range",
        "module m\n\
         resource Cell\n    required value: int\n\
         store ^cells(x: int, y: int): Cell\n\n\
         fn f(lo: int, hi: int)\n    for y in ^cells(1, lo..hi)\n        var typed_y: int = y\n",
    );
    assert_clean(&report);
}

#[test]
fn root_identity_splice_range_argument_is_rejected() {
    let found = check_module(
        "root-identity-splice-range",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(lo: Id(^books), hi: Id(^books))\n    for id in ^books(lo..hi)\n        print(id)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn trailing_range_keyed_layer_argument_checks_clean() {
    let report = check_module_report(
        "trailing-layer-range",
        "module m\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: int, lo: int, hi: int)\n    for pos in ^books(id).tags(lo..hi)\n        var typed_pos: int = pos\n",
    );
    assert_clean(&report);
}

#[test]
fn enum_store_keyspace_range_argument_is_rejected() {
    let found = check_module(
        "enum-root-range",
        "module m\n\
         enum Axis\n    x\n    y\n\
         resource Cell\n    required value: int\n\
         store ^cells(axis: Axis): Cell\n\n\
         fn f(lo: Axis)\n    for axis in ^cells(lo..)\n        print(axis)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn enum_keyed_layer_range_argument_is_rejected() {
    let found = check_module(
        "enum-layer-range",
        "module m\n\
         enum Label\n    one\n    two\n\
         resource Book\n    tags(label: Label): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books), lo: Label)\n    for label in ^books(id).tags(lo..)\n        print(label)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn enum_index_range_arguments_check_endpoints_consistently() {
    let report = check_module_report(
        "enum-index-range",
        "module m\n\
         enum Status\n    draft\n    published\n\
         resource Post\n    status: Status\n\
         store ^posts(id: int): Post\n\n    index byStatus(status, id)\n\n\
         fn f(lo: Status, hi: Status)\n    for id in ^posts.byStatus(lo..hi)\n        var typed: Id(^posts) = id\n    for id in ^posts.byStatus(lo..)\n        var from_typed: Id(^posts) = id\n    for id in ^posts.byStatus(..hi)\n        var before_typed: Id(^posts) = id\n    for id in ^posts.byStatus(..=hi)\n        var through_typed: Id(^posts) = id\n",
    );
    assert_clean(&report);
}

#[test]
fn collection_wrappers_accept_saved_key_range_arguments() {
    let report = check_module_report(
        "wrapped-key-ranges",
        "module m\n\
         resource Cell\n    required value: int\n\
         store ^cells(x: int, y: int): Cell\n\
         resource Book\n    required title: string\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\
         resource Post\n    published: int\n    required title: string\n\
         store ^posts(id: int): Post\n\n    index byDate(published, id)\n\n\
         fn f(book: Id(^books), lo: int, hi: int)\n    for y in keys(^cells(1, lo..hi))\n        print(y)\n    for cell in values(^cells(1, lo..hi))\n        var v: int = cell.value\n    for y, cell in entries(^cells(1, lo..hi))\n        var typed_y: int = y\n        var entry_v: int = cell.value\n    for pos in keys(^books(book).tags(lo..hi))\n        print(pos)\n    for pos, tag in entries(^books(book).tags(lo..hi))\n        var typed_pos: int = pos\n        var s: string = tag\n    for post in keys(^posts.byDate(lo..hi))\n        print(post)\n",
    );
    assert_clean(&report);
}

#[test]
fn saved_key_range_calls_are_rejected_in_value_position() {
    let found = check_module(
        "range-call-value-position",
        "module m\n\
         resource Cell\n    required value: int\n\
         store ^cells(x: int, y: int): Cell\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books), lo: int, hi: int)\n    const row = ^cells(1, lo..hi)\n    const tag = ^books(id).tags(lo..hi)\n",
        "check.range_value",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn exists_accepts_index_range_but_rejects_root_and_layer_ranges() {
    let clean = check_module_report(
        "exists-index-range",
        "module m\n\
         resource Post\n    published: int\n\
         store ^posts(id: int): Post\n\n    index byDate(published, id)\n\n\
         fn f(lo: int, hi: int): bool\n    return exists(^posts.byDate(lo..hi))\n",
    );
    assert_clean(&clean);

    let root = check_module(
        "exists-root-range",
        "module m\n\
         resource Cell\n    required value: int\n\
         store ^cells(x: int, y: int): Cell\n\n\
         fn f(lo: int, hi: int): bool\n    return exists(^cells(1, lo..hi))\n",
        "check.call_argument",
    );
    assert_eq!(root.len(), 1, "{root:#?}");

    let layer = check_module(
        "exists-layer-range",
        "module m\n\
         resource Book\n    tags(pos: int): string\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books), lo: int, hi: int): bool\n    return exists(^books(id).tags(lo..hi))\n",
        "check.call_argument",
    );
    assert_eq!(layer.len(), 1, "{layer:#?}");
}

#[test]
fn inclusive_open_end_range_key_argument_is_rejected() {
    let found = check_module(
        "inclusive-open-end-key-range",
        "module m\n\
         resource Post\n    published: int\n    required title: string\n\
         store ^posts(id: int): Post\n\n    index byDate(published, id)\n\n\
         fn f(start: int)\n    for id in ^posts.byDate(start..=)\n        print(id)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn non_trailing_range_index_argument_is_rejected() {
    let found = check_module(
        "non-trailing-index-range",
        "module m\n\
         resource Post\n    published: int\n    shelf: string\n\
         store ^posts(id: int): Post\n\n    index byDateShelf(published, shelf, id)\n\n\
         fn f(start: int, end: int)\n    for shelf in ^posts.byDateShelf(start..end, \"news\")\n        print(shelf)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn pre_identity_partial_index_range_argument_is_rejected() {
    let found = check_module(
        "pre-identity-partial-index-range",
        "module m\n\
         resource Book\n    author: int\n    shelf: int\n\
         store ^books(id: int): Book\n\n    index byAuthorShelf(author, shelf, id)\n\n\
         fn f(lo: int, hi: int)\n    for shelf in ^books.byAuthorShelf(lo..hi)\n        print(shelf)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn identity_field_index_rejects_wrong_store_identity_argument() {
    let found = check_module(
        "identity-index-wrong-store",
        "module m\n\
         resource Author\n    required name: string\n\
         store ^authors(id: int): Author\n\
         resource Publisher\n    required name: string\n\
         store ^publishers(id: int): Publisher\n\
         resource Book\n    required title: string\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n    index byAuthor(authorId, id)\n\n\
         fn f(publisher: Id(^publishers))\n    for id in ^books.byAuthor(publisher)\n        print($\"{id}\")\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn identity_index_component_rejects_range_argument() {
    let found = check_module(
        "identity-index-range",
        "module m\n\
         resource Author\n    required name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n    required title: string\n    author: Id(^authors)\n\
         store ^books(id: int): Book\n\n    index byAuthor(author, id)\n\n\
         fn f(lo: Id(^authors), hi: Id(^authors))\n    for id in ^books.byAuthor(lo..hi)\n        print(id)\n",
        "check.key_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn identity_yielding_index_branches_bind_identity_and_resource_pairs() {
    let report = check_module_report(
        "index-pair-loop",
        "module m\n\
         resource Book\n    required title: string\n    author: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n    index byAuthorShelf(author, shelf, id)\n\n\
         fn f()\n    \
         for id, book in ^books.byAuthorShelf(\"ann\", \"fiction\")\n        var typed_id: Id(^books) = id\n        var typed_title: string = book.title\n    \
         for exact_id, exact_book in ^books.byAuthorShelf(\"ann\", \"fiction\", 1)\n        var exact_typed: Id(^books) = exact_id\n        var exact_title: string = exact_book.title\n",
    );
    assert_clean(&report);
}

#[test]
fn partial_non_unique_index_branches_accept_two_name_loops() {
    // A partial prefix over a non-unique index yields the store identity, so a
    // two-name loop pairs that identity with the materialized record.
    let report = check_module_report(
        "partial-index-pair-loop",
        "module m\n\
         resource Book\n    author: string\n    shelf: string\n\
         store ^books(id: int): Book\n\n    index byAuthorShelf(author, shelf, id)\n\n\
         fn f()\n    for id, book in ^books.byAuthorShelf(\"ann\")\n        var typed_id: Id(^books) = id\n        print($\"{book.shelf}\")\n",
    );
    assert_clean(&report);
}

#[test]
fn singleton_root_keys_do_not_bind_generated_identities() {
    let found = check_module(
        "singleton-root-keys",
        "module m\n\
         resource Settings\n    value: int\n\
         store ^settings: Settings\n\n\
         fn f()\n    for id in keys(^settings)\n        var n = id + 1\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn supported_collection_wrappers_bind_their_documented_shapes() {
    let report = check_module_report(
        "collection-wrapper-shapes",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    for id in keys(^books)\n        var typed: Id(^books) = id\n    for book in values(^books)\n        var title: string = book.title\n    for id, book in entries(^books)\n        var typed: Id(^books) = id\n        var title: string = book.title\n    for book in reversed(values(^books))\n        var title: string = book.title\n    for id, book in reversed(entries(^books))\n        var reversed_typed: Id(^books) = id\n        var reversed_title: string = book.title\n",
    );
    assert_clean(&report);
}

#[test]
fn layer_key_traversal_binds_declared_key_types() {
    let report = check_module_report(
        "layer-key-traversal-types",
        "module m\n\
         resource Run\n    terms: sequence[string]\n    amounts(pos: int): decimal\n\
         store ^runs(id: int): Run\n\n\
         fn f(id: Id(^runs))\n    for pos in keys(^runs(id).terms)\n        const first: bool = pos == 1\n    for pos, amount in entries(^runs(id).amounts)\n        const numbered: bool = pos == 1\n        const total: decimal = amount + 1.0\n",
    );
    assert_clean(&report);
}

#[test]
fn composite_root_traversal_binds_addressable_identities() {
    let report = check_module_report(
        "composite-root-traversal-id",
        "module m\n\
         resource Cell\n    required v: int\n\
         store ^cells(x: int, y: int): Cell\n\n\
         fn f()\n    for id, cell in ^cells\n        const typed: Id(^cells) = id\n        const copy: int = cell.v\n",
    );
    assert_clean(&report);
}

#[test]
fn index_branches_reject_value_materialization_wrappers() {
    for wrapper in ["values", "entries"] {
        let found = check_module(
            &format!("index-{wrapper}-unsupported"),
            &format!(
                "module m\n\
                 resource Book\n    shelf: string\n\
                 store ^books(id: int): Book\n\n    index byShelf(shelf, id)\n\n\
                 fn f()\n    for item in {wrapper}(^books.byShelf(\"fiction\"))\n        print($\"{{item}}\")\n",
            ),
            "check.collection_unsupported",
        );
        assert_eq!(found.len(), 1, "{wrapper}: {found:#?}");
    }
}

#[test]
fn reversed_saved_collection_expressions_type_element_sequences() {
    let found = check_module(
        "reversed-saved-expressions",
        "module m\n\
         resource Book\n    required title: string\n    tags: sequence[string]\n\
         store ^books(id: int): Book\n\n\
         fn f(id: Id(^books))\n    const ids = reversed(^books)\n    for bookId in ids\n        var typed: Id(^books) = bookId\n    const positions = reversed(^books(id).tags)\n    for pos in positions\n        var numbered: int = pos\n    const books = reversed(values(^books))\n    for book in books\n        var bad = book.title + 1\n    const tags = reversed(values(^books(id).tags))\n    for tag in tags\n        var also_bad = tag + 1\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 2, "{found:#?}");
}

#[test]
fn unresolved_calls_are_suppressed_when_a_module_fails_to_parse() {
    // Module `a` has a lexical error (a leading tab), so it is excluded from the
    // program; a call to `a::helper` in clean module `b` must not be reported as
    // unresolved — the definition exists, the project just did not fully parse.
    let root = temp_project("call-incomplete", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\tpub fn helper()\n    return\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\nuse a\nfn caller()\n    a::helper()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert!(
        with_code(&report, "check.unresolved_call").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}
