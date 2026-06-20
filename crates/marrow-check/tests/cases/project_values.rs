use crate::support;
use marrow_check::{DiagnosticPayload, MarrowType, check_project};

use support::{
    assert_clean, check_module, check_module_report, check_script, config, temp_project, with_code,
    write,
};

#[test]
fn an_over_range_int_literal_is_flagged_at_check_time() {
    // `99999999999999999999999999` exceeds i64; the runtime would reject it as
    // run.overflow, so the checker flags it too.
    let found = check_script(
        "int-literal-overflow",
        "fn f()\n    const x: int = 99999999999999999999999999\n",
        "check.literal_range",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_in_range_int_literal_is_not_flagged() {
    // i64::MAX checks clean.
    let found = check_script(
        "int-literal-max",
        "fn f()\n    const x: int = 9223372036854775807\n",
        "check.literal_range",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn i64_min_is_in_range_only_when_negated() {
    // `i64::MIN` (`-9223372036854775808`) is a valid value, so it checks clean even
    // though its bare magnitude is `i64::MAX + 1`. The same magnitude unnegated, and
    // a magnitude one past it negated, are out of range.
    let negated = check_script(
        "int-literal-min",
        "fn f()\n    const m: int = -9223372036854775808\n",
        "check.literal_range",
    );
    assert!(negated.is_empty(), "{negated:#?}");

    let unnegated = check_script(
        "int-literal-min-magnitude",
        "fn f()\n    const m: int = 9223372036854775808\n",
        "check.literal_range",
    );
    assert_eq!(unnegated.len(), 1, "{unnegated:#?}");

    let below_min = check_script(
        "int-literal-below-min",
        "fn f()\n    const m: int = -9223372036854775809\n",
        "check.literal_range",
    );
    assert_eq!(below_min.len(), 1, "{below_min:#?}");
}

#[test]
fn an_over_envelope_decimal_literal_is_flagged_at_check_time() {
    // 35 significant digits exceeds the 34-digit decimal envelope.
    let found = check_script(
        "decimal-literal-overflow",
        "fn f()\n    const d: decimal = 1.2345678901234567890123456789012345\n",
        "check.literal_range",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_in_range_decimal_literal_is_not_flagged() {
    // 34 significant digits is exactly at the envelope, and a long trailing-zero
    // fraction normalizes back into range — neither is flagged.
    let found = check_script(
        "decimal-literal-ok",
        "fn f()\n\
         \x20   const d: decimal = 1.234567890123456789012345678901234\n\
         \x20   const z: decimal = 0.000000000000000000000000000000000000\n",
        "check.literal_range",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_over_range_module_const_literal_is_flagged_at_check_time() {
    // A module-level `const` initializer is range-checked like a local one.
    let found = check_module(
        "module-const-literal-overflow",
        "module m\nconst BIG: int = 99999999999999999999999999\n",
        "check.literal_range",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_var_initializer_of_the_wrong_type() {
    // `x` is declared `int` but initialized with a string.
    let found = check_script(
        "init-var",
        "fn f()\n    var x: int = \"hi\"\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_const_initializer_of_the_wrong_type() {
    // `x` is declared `bool` but initialized with an int.
    let found = check_script(
        "init-const",
        "fn f()\n    const x: bool = 1\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_an_assignment_to_a_local_of_the_wrong_type() {
    // `x` is an int local; assigning a string is a mismatch.
    let found = check_script(
        "assign-local",
        "fn f()\n    var x: int = 1\n    x = \"hi\"\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_saved_field_write_of_the_wrong_type() {
    // `currentVersion` is `int`, so writing a string is a mismatch.
    let found = check_module(
        "assign-saved",
        "module m\n\
         resource Book\n    currentVersion: int\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    ^books(1).currentVersion = \"hi\"\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn well_typed_assignments_and_initializers_are_not_flagged() {
    // Each binding and assignment matches the declared/known type.
    let found = check_script(
        "assign-ok",
        "fn f()\n    var x: int = 1\n    x = 2\n    const s: string = \"a\"\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_value_into_a_typed_place_is_flagged() {
    // Strict typing: `mystery()` does not resolve, so storing it into the concrete
    // `int` place is a `check.untyped_value` error — convert or define it. It is
    // not a `check.assignment_type` mismatch, so one analysis must surface the
    // untyped-value code while leaving the primitive-mismatch code unraised.
    let root = temp_project("assign-unknown", |root| {
        write(
            root,
            "src/app.mw",
            "fn f()\n    var x: int = 1\n    x = mystery()\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_eq!(
        with_code(&report, "check.untyped_value").len(),
        1,
        "{:#?}",
        report.diagnostics
    );
    assert!(
        with_code(&report, "check.assignment_type").is_empty(),
        "{:#?}",
        report.diagnostics
    );
}

#[test]
fn a_typed_initializer_with_an_unresolved_value_is_flagged() {
    // A typed `const` initializer whose value has no known type is flagged.
    let found = check_script(
        "init-unknown",
        "fn f()\n    const n: int = mystery()\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_an_identity_place_is_not_flagged() {
    // `nextId(^books)` is typed `Id(^books)`, not `unknown`, so the initializer is the
    // nominal match — this guards the `const id: Id(^books) = nextId(^books)` shape
    // against a false untyped-value error.
    let found = check_module(
        "untyped-identity",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    const id: Id(^books) = nextId(^books)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_typed_field_accepts_an_identity_of_that_store() {
    // A saved field typed `Id(^authors)` is a reference: assigning a real
    // `Id(^authors)` is the nominal match, so nothing is flagged.
    let found = check_module(
        "ref-field-ok",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    ^books(1).authorId = nextId(^authors)\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_identity_typed_field_rejects_a_wrong_store_identity() {
    // Assigning a `Id(^books)` into an `Id(^authors)` field is the nominal mismatch a
    // typed reference forbids.
    let found = check_module(
        "ref-field-wrong-store",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    ^books(1).authorId = nextId(^books)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_identity_typed_field_rejects_a_raw_scalar() {
    // A bare `int` is not an identity; store identity values are produced by
    // operations such as `nextId(^authors)`.
    let found = check_module(
        "ref-field-raw-scalar",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n\
         fn f()\n    ^books(1).authorId = 7\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_an_identity_field_is_an_untyped_value() {
    // A dynamic `unknown` parameter stored into an `Id(^authors)` field is the
    // foreign-value hazard: a single raw key is a structurally valid identity
    // encoding, so `data integrity` cannot catch it later. Strict typing rejects the
    // unconverted value the same way a scalar place does.
    let found = check_module(
        "ref-field-untyped",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n\
         fn put(x: unknown)\n    ^books(1).authorId = x\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn nextid_into_an_identity_field_is_not_an_untyped_value() {
    // `nextId(^authors)` is typed `Id(^authors)`, not `unknown`, so assigning it into
    // an `Id(^authors)` field is the nominal match — never the untyped-value path.
    let found = check_module(
        "ref-field-nextid-ok",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    authorId: Id(^authors)\n\
         store ^books(id: int): Book\n\n\
         fn put()\n    ^books(1).authorId = nextId(^authors)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn multiple_stores_over_one_resource_keep_distinct_identities() {
    let report = check_module_report(
        "two-stores-one-resource",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\
         store ^archivedBooks(id: int): Book\n\n\
         fn freshBook(): Id(^books)\n    return nextId(^books)\n\
         fn freshArchived(): Id(^archivedBooks)\n    return nextId(^archivedBooks)\n\
         fn wrong(): Id(^books)\n    return nextId(^archivedBooks)\n",
    );
    let return_type = with_code(&report, "check.return_type");
    assert_eq!(return_type.len(), 1, "{:#?}", report.diagnostics);
    assert_eq!(
        return_type[0].payload,
        DiagnosticPayload::TypeMismatch {
            expected: MarrowType::Identity("books".into()),
            found: MarrowType::Identity("archivedBooks".into()),
        },
        "{return_type:#?}"
    );
    assert!(
        with_code(&report, "check.untyped_value").is_empty(),
        "`nextId` over each declared store must be typed: {:#?}",
        report.diagnostics
    );
}

#[test]
fn same_named_resources_use_their_own_module_shape() {
    let root = temp_project("same-name-resource-shape", |root| {
        write(
            root,
            "src/a.mw",
            "module a\n\
             resource Book\n    title: int\n\
             store ^aBooks(id: int): Book\n",
        );
        write(
            root,
            "src/b.mw",
            "module b\n\
             resource Book\n    title: string\n\
             store ^bBooks(id: int): Book\n\
             fn f(): string\n    const b = Book(title: \"ok\")\n    return b.title\n",
        );
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_clean(&report);
}

#[test]
fn identity_type_must_name_a_declared_store() {
    let found = check_module(
        "missing-id-store",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\
         resource Book\n    author: Id(^authors)\n    missing: Id(^missing)\n\
         store ^books(id: int): Book\n",
        "check.unknown_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].payload,
        DiagnosticPayload::UnknownType(marrow_schema::Type::Identity("missing".into()))
    );
}

#[test]
fn an_unknown_value_into_a_whole_resource_is_an_untyped_value() {
    // `^books(1) = x` writes a whole `Book`. A dynamic `unknown` value carries no
    // type, so its fields could spill a raw scalar or a foreign identity into a
    // typed (identity) field — a structurally valid encoding the runtime cannot
    // later distinguish. A whole resource is a concrete typed place, so the value
    // must be converted into a `Book` first.
    let found = check_module(
        "whole-resource-untyped",
        "module m\n\
         resource Book\n    authorId: int\n\
         store ^books(id: int): Book\n\n\
         fn put(x: unknown)\n    ^books(1) = x\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn an_unknown_value_into_a_whole_group_entry_is_an_untyped_value() {
    // `^books(1).chapters(0) = x` writes a whole group entry. Like a whole
    // resource, the entry is a concrete typed record place, so a dynamic `unknown`
    // value (which could land a raw scalar or foreign identity in a typed field)
    // must be converted first.
    let found = check_module(
        "whole-group-entry-untyped",
        "module m\n\
         resource Book\n\
         \x20\x20\x20\x20title: string\n\
         \x20\x20\x20\x20chapters(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20title: string\n\
         store ^books(id: int): Book\n\n\
         fn put(x: unknown)\n    ^books(1).chapters(0) = x\n",
        "check.untyped_value",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_typed_whole_resource_write_is_not_an_untyped_value() {
    // A whole-resource write of a value already typed as the resource (a read
    // `^books(2)`, a constructed `Book(...)`, or a `Book`-typed local) is the
    // nominal match — never the untyped-value path.
    let found = check_module(
        "whole-resource-typed-ok",
        "module m\n\
         resource Book\n    required title: string\n\
         store ^books(id: int): Book\n\n\
         fn copy()\n    ^books(1) = ^books(2)\n\n\
         fn construct()\n    ^books(1) = Book(title: \"hi\")\n\n\
         fn local()\n    var b: Book\n    b.title = \"hi\"\n    ^books(1) = b\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_whole_group_entry_copy_read_requires_read_site_resolution() {
    let report = check_module_report(
        "whole-group-entry-typed-ok",
        "module m\n\
         resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20chapters(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         store ^books(id: int): Book\n\n\
         fn local()\n    var b: Book\n    b.title = \"v1\"\n    ^books(1).chapters(0) = b\n\n\
         fn copy()\n    ^books(1).chapters(1) = ^books(1).chapters(0)\n",
    );
    let found = with_code(&report, "check.bare_maybe_present_read");
    assert_eq!(found.len(), 1, "{:#?}", report.diagnostics);
}

#[test]
fn a_group_entry_does_not_flow_as_a_whole_resource() {
    let source = "module m\n\
         resource Book\n\
         \x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         store ^books(id: int): Book\n\n\
         fn takesBook(book: Book)\n    print(book.title)\n\n\
         fn returnsBook(id: Id(^books)): Book\n    for versionKey, version in ^books(id).versions\n        return version\n    return ^books(id)\n\n\
         fn pass(id: Id(^books))\n    for versionKey, version in ^books(id).versions\n        takesBook(version)\n\n\
         fn assign(id: Id(^books))\n    for versionKey, version in ^books(id).versions\n        var book: Book = version\n";

    let returns = check_module(
        "group-entry-not-resource-return",
        source,
        "check.return_type",
    );
    assert_eq!(returns.len(), 1, "{returns:#?}");
    let args = check_module(
        "group-entry-not-resource-arg",
        source,
        "check.call_argument",
    );
    assert_eq!(args.len(), 1, "{args:#?}");
    let assignments = check_module(
        "group-entry-not-resource-assignment",
        source,
        "check.assignment_type",
    );
    assert_eq!(assignments.len(), 1, "{assignments:#?}");
}

#[test]
fn a_whole_group_entry_write_rejects_a_different_group_layer() {
    let found = check_module(
        "whole-group-entry-different-layer",
        "module m\n\
         resource Book\n\
         \x20\x20\x20\x20chapters(pos: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         \x20\x20\x20\x20versions(version: int)\n\
         \x20\x20\x20\x20\x20\x20\x20\x20required title: string\n\
         store ^books(id: int): Book\n\n\
         fn copy()\n    ^books(1).chapters(1) = ^books(1).versions(1)\n",
        "check.assignment_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn equality_on_two_identities_of_the_same_store_types_bool() {
    // Two `Id(^authors)` values compare with `==`; the result is `bool`, so no
    // operator diagnostic is raised.
    let found = check_module(
        "ref-eq-same-store",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         fn f(): bool\n    return nextId(^authors) == nextId(^authors)\n",
        "check.operator_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn equality_across_resource_identities_is_an_operator_error() {
    // `==` between an `Id(^authors)` and a `Id(^books)` is a nominal category error.
    let found = check_module(
        "ref-eq-cross-resource",
        "module m\n\
         resource Author\n    name: string\n\
         store ^authors(id: int): Author\n\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): bool\n    return nextId(^authors) == nextId(^books)\n",
        "check.operator_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_self_referencing_identity_field_accepts_its_own_identity() {
    // A field typed as its owning store identity is a valid self reference.
    let found = check_module(
        "ref-self",
        "module m\n\
         resource Person\n    managerId: Id(^people)\n\
         store ^people(id: int): Person\n\n\
         fn f()\n    ^people(1).managerId = nextId(^people)\n",
        "check.assignment_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn two_unwritten_next_ids_used_as_distinct_keys_warn() {
    // Both `a` and `b` allocate from `^docs` with no write to `^docs` between the two
    // `nextId` calls, so they hold the same value (max + 1). Writing each as its own
    // record key inserts the same record twice — a silent overwrite. The checker warns.
    let found = check_module(
        "nextid-collision",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n\
         \x20   const a = nextId(^docs)\n\
         \x20   const b = nextId(^docs)\n\
         \x20   ^docs(a).title = \"x\"\n\
         \x20   ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(
        found[0].severity,
        marrow_syntax::Severity::Warning,
        "{found:#?}"
    );
}

#[test]
fn allocate_then_write_interleaved_does_not_warn() {
    // The safe pattern: write `^docs(a)` before allocating `b`. The intervening write
    // advances the allocation, so `b` is a fresh, distinct id. No collision, no warning.
    let found = check_module(
        "nextid-interleaved",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n\
         \x20   const a = nextId(^docs)\n\
         \x20   ^docs(a).title = \"x\"\n\
         \x20   const b = nextId(^docs)\n\
         \x20   ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_single_next_id_does_not_warn() {
    // One allocation written once is the ordinary, correct shape.
    let found = check_module(
        "nextid-single",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n\
         \x20   const a = nextId(^docs)\n\
         \x20   ^docs(a).title = \"x\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn two_next_ids_for_different_stores_do_not_warn() {
    // `a` and `b` allocate from different stores, so their values are independent and
    // never collide, even with no intervening write to either.
    let found = check_module(
        "nextid-distinct-stores",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         resource Tag\n    name: string\n\
         store ^tags(id: int): Tag\n\n\
         fn f()\n\
         \x20   const a = nextId(^docs)\n\
         \x20   const b = nextId(^tags)\n\
         \x20   ^docs(a).title = \"x\"\n\
         \x20   ^tags(b).name = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn interleaved_writes_inside_a_transaction_do_not_warn() {
    // A transaction does not make two equal ids distinct, but interleaving the writes
    // still advances allocation between allocations, so the transactional form is the
    // safe one and must not warn.
    let found = check_module(
        "nextid-transaction-interleaved",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n\
         \x20   transaction\n\
         \x20       const a = nextId(^docs)\n\
         \x20       ^docs(a).title = \"x\"\n\
         \x20       const b = nextId(^docs)\n\
         \x20       ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn allocations_in_mutually_exclusive_branches_do_not_warn() {
    // `a` and `b` live in disjoint branches, so they are never both written in one
    // run. The two writes cannot collide; no warning.
    let found = check_module(
        "nextid-branches",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f(flag: bool)\n\
         \x20   if flag\n\
         \x20       const a = nextId(^docs)\n\
         \x20       ^docs(a).title = \"x\"\n\
         \x20   else\n\
         \x20       const b = nextId(^docs)\n\
         \x20       ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_single_allocation_written_each_loop_iteration_does_not_warn() {
    // Each iteration allocates and writes one id; the write advances allocation before
    // the next iteration allocates, so no two written ids are ever equal.
    let found = check_module(
        "nextid-loop",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n\
         \x20   for n in 1..3\n\
         \x20       const a = nextId(^docs)\n\
         \x20       ^docs(a).title = \"x\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn two_allocations_inside_one_loop_body_warn() {
    // Within a single iteration, `a` and `b` allocate with no write between them, so
    // they are equal and writing both is a collision — the loop does not excuse it.
    let found = check_module(
        "nextid-loop-collision",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f()\n\
         \x20   for n in 1..3\n\
         \x20       const a = nextId(^docs)\n\
         \x20       const b = nextId(^docs)\n\
         \x20       ^docs(a).title = \"x\"\n\
         \x20       ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn hoisted_allocations_written_in_mutually_exclusive_branches_do_not_warn() {
    // Both ids are allocated up front, then each is written in a different arm of one
    // `if`. The two writes are on disjoint paths and never both run, so a write in one
    // arm must not be seen as a colliding sibling of a write in the other arm.
    let found = check_module(
        "nextid-hoisted-branches",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f(flag: bool)\n\
         \x20   const a = nextId(^docs)\n\
         \x20   const b = nextId(^docs)\n\
         \x20   if flag\n\
         \x20       ^docs(a).title = \"x\"\n\
         \x20   else\n\
         \x20       ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn hoisted_allocations_written_in_mutually_exclusive_match_arms_do_not_warn() {
    // The same disjoint-path reasoning holds across `match` arms.
    let found = check_module(
        "nextid-hoisted-match",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f(n: int)\n\
         \x20   const a = nextId(^docs)\n\
         \x20   const b = nextId(^docs)\n\
         \x20   match n\n\
         \x20       0\n\
         \x20           ^docs(a).title = \"x\"\n\
         \x20       _\n\
         \x20           ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn two_writes_of_one_cohort_in_a_single_arm_still_warn() {
    // A branch must not hide a real collision: when both same-cohort ids are written
    // on a common path inside one arm, the second write overwrites the first.
    let found = check_module(
        "nextid-branch-real-collision",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn f(flag: bool)\n\
         \x20   const a = nextId(^docs)\n\
         \x20   const b = nextId(^docs)\n\
         \x20   if flag\n\
         \x20       ^docs(a).title = \"x\"\n\
         \x20       ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_user_function_write_between_allocations_advances_the_cohort() {
    // `writer()` writes `^docs` through its effect closure, so `b` allocated after the
    // call is a fresh id distinct from `a`. The unmodeled-but-known write must suppress
    // the warning, never invent one.
    let found = check_module(
        "nextid-writer-between",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn writer()\n\
         \x20   const c = nextId(^docs)\n\
         \x20   ^docs(c).title = \"w\"\n\n\
         fn f()\n\
         \x20   const a = nextId(^docs)\n\
         \x20   writer()\n\
         \x20   const b = nextId(^docs)\n\
         \x20   ^docs(a).title = \"x\"\n\
         \x20   ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_user_function_write_in_value_position_advances_the_cohort() {
    // The same write-between-allocations suppression holds when the writer call is in
    // value position (`const x = writer()`), not just a bare statement.
    let found = check_module(
        "nextid-writer-value",
        "module m\n\
         resource Doc\n    title: string\n\
         store ^docs(id: int): Doc\n\n\
         fn writer(): int\n\
         \x20   const c = nextId(^docs)\n\
         \x20   ^docs(c).title = \"w\"\n\
         \x20   return 0\n\n\
         fn f()\n\
         \x20   const a = nextId(^docs)\n\
         \x20   const used = writer()\n\
         \x20   const b = nextId(^docs)\n\
         \x20   ^docs(a).title = \"x\"\n\
         \x20   ^docs(b).title = \"y\"\n",
        "check.next_id_collision",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unknown_value_into_an_unknown_place_is_not_flagged() {
    // `unknown` is the explicit dynamic opt-out: storing an unresolved value into
    // an `unknown`-typed place is allowed.
    let found = check_script(
        "untyped-into-unknown",
        "fn f()\n    var raw: unknown = mystery()\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn rejects_a_return_of_the_wrong_type() {
    // The function is declared to return `int`, but `true` is a bool.
    let found = check_script(
        "ret-type",
        "fn f(): int\n    return true\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn rejects_a_returned_local_of_the_wrong_type() {
    // `s` is inferred `string` from its initializer, but `f` returns `int`.
    let found = check_script(
        "ret-local",
        "fn f(): int\n    const s = \"hi\"\n    return s\n",
        "check.return_type",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn correct_returns_are_not_flagged() {
    // Each returned value matches the function's declared return type.
    let found = check_script(
        "ret-ok",
        "fn f(): int\n    return 1\n\nfn g(b: bool): bool\n    return b\n",
        "check.return_type",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_return_of_an_unresolved_value_into_a_typed_return_is_flagged() {
    // Strict typing: `mystery()` has no known type, but `f` returns `int`, so the
    // return is a `check.untyped_value` error, not a `check.return_type` mismatch —
    // one analysis must raise the untyped-value code and leave the mismatch unraised.
    let root = temp_project("ret-unknown", |root| {
        write(root, "src/app.mw", "fn f(): int\n    return mystery()\n");
    });
    let (report, _program) = check_project(&root, &config()).expect("check");
    assert_eq!(
        with_code(&report, "check.untyped_value").len(),
        1,
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
fn a_return_of_an_unresolved_value_into_an_identity_return_is_not_flagged() {
    // A non-primitive return type (an identity) is excluded from strict
    // untyped-value checking — guards the sample's `return nextId(...)`-style code.
    let found = check_module(
        "ret-identity",
        "module m\n\
         resource Book\n    title: string\n\
         store ^books(id: int): Book\n\n\
         fn f(): Id(^books)\n    return nextId(^books)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn a_unique_index_lookup_types_as_the_resource_identity() {
    // `^books.byIsbn(isbn)` reads back the owning identity, so it types as
    // `Id(^books)` — not `Unknown`. Returned where `int` is expected, that is a
    // typed value (a non-primitive identity), so strict untyped-value checking
    // does not fire.
    let found = check_module(
        "unique-index-identity",
        "module m\n\
         resource Book\n    title: string\n    isbn: string\n\
         store ^books(id: int): Book\n\n    index byIsbn(isbn) unique\n\n\
         fn f(isbn: string): int\n    return ^books.byIsbn(isbn)\n",
        "check.untyped_value",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn std_log_error_of_an_error_constructor_checks_clean() {
    // std::log::error takes an Error; the Error(...) constructor must type AS Error
    // (not Unknown), so the canonical log::error(Error(...)) is not a false
    // check.untyped_value / check.call_argument.
    let src =
        "module m\nuse std::log\nfn f()\n    log::error(Error(code: \"x.y\", message: \"m\"))\n";
    assert!(
        check_module("std-log-error-untyped", src, "check.untyped_value").is_empty(),
        "Error(...) must type as Error, not Unknown"
    );
    assert!(
        check_module("std-log-error-arg", src, "check.call_argument").is_empty(),
        "log::error(Error(...)) is the spec-canonical call"
    );
}

#[test]
fn an_unsupported_string_escape_is_flagged_at_check_time() {
    let found = check_script(
        "string-escape-unsupported",
        "fn f()\n    const s: string = \"x\\q\"\n",
        "check.string_escape",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn supported_string_escapes_check_clean() {
    let found = check_script(
        "string-escape-supported",
        "fn f()\n    const s: string = \"a\\\\b\\\"c\\nd\\re\\tf\"\n",
        "check.string_escape",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unsupported_bytes_escape_is_flagged_at_check_time() {
    let found = check_script(
        "bytes-escape-unsupported",
        "fn f()\n    const b: bytes = b\"x\\q\"\n",
        "check.bytes_escape",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn a_truncated_bytes_hex_escape_is_flagged_at_check_time() {
    let found = check_script(
        "bytes-escape-truncated-hex",
        "fn f()\n    const b: bytes = b\"\\x1\"\n",
        "check.bytes_escape",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}

#[test]
fn supported_bytes_escapes_check_clean() {
    let found = check_script(
        "bytes-escape-supported",
        "fn f()\n    const b: bytes = b\"\\xff\\n\\\\\\\"\\r\\t\"\n",
        "check.bytes_escape",
    );
    assert!(found.is_empty(), "{found:#?}");
}

#[test]
fn an_unsupported_escape_in_an_interpolation_text_segment_is_flagged() {
    let found = check_script(
        "string-escape-interpolation",
        "fn f()\n    print($\"bad\\q here\")\n",
        "check.string_escape",
    );
    assert_eq!(found.len(), 1, "{found:#?}");
}
