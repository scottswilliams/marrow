mod support;

use marrow_check::check_project;

use support::{config, temp_project, write};

// --- Equality, coalesce, and unary over concrete non-scalar types ---

/// Two resources and a sequence-yielding helper, for the operator-soundness tests
/// below. `Book` and `Magazine` are distinct nominal resources with the same key
/// shape, so an identity of one is byte-identical to an identity of the other yet
/// must not compare equal.
const OPERATOR_OPERANDS: &str = "module shelf::ops\n\
     resource Book\n\
     \x20   required title: string\n\
     store ^books(id: int): Book\n\
     resource Magazine\n\
     \x20   required title: string\n\
     store ^magazines(id: int): Magazine\n";

/// Compare two whole records of different resources with `==`. Equality is not
/// defined over whole records, so this is `check.operator_type`, not a silent
/// fall-through to a `bool` result.
#[test]
fn resource_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-resource", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Book, m: Magazine): bool\n\
                 \x20   return b == m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare a whole record against a scalar with `==`. A record is not a scalar, so
/// the comparison is `check.operator_type`.
#[test]
fn resource_against_scalar_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-resource-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Book, n: int): bool\n\
                 \x20   return b == n\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare two sequences with `==`. Equality is not defined over sequences, so the
/// comparison is `check.operator_type`.
#[test]
fn sequence_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-sequence", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(xs: sequence[int], ys: sequence[int]): bool\n\
                 \x20   return xs == ys\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare identities of different resources with `==`. They share a key shape but
/// name different resources, so equality across them is `check.operator_type`.
#[test]
fn cross_resource_identity_equality_is_an_operator_type_error() {
    let root = temp_project("program-eq-id-cross", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books), m: Id(^magazines)): bool\n\
                 \x20   return b == m\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Compare two identities of the *same* store with `==`. Identity equality is
/// usable, so the comparison checks clean and types to `bool` — a function that
/// returns that comparison from a `: bool` body has no diagnostic.
#[test]
fn same_store_identity_equality_checks_clean() {
    let root = temp_project("program-eq-id-same", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(a: Id(^books), b: Id(^books)): bool\n\
                 \x20   return a == b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// A raw scalar `==` scalar comparison checks clean.
#[test]
fn raw_scalar_equality_still_checks_clean() {
    let root = temp_project("program-eq-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(a: int, b: int): bool\n\
                 \x20   return a == b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

/// Coalescing two identities of different resources with `??` is a nominal
/// mismatch reported as `check.operator_type`: the unique-index read on the left
/// yields a `Id(^books)`, and a `Id(^magazines)` default cannot stand in for it. The
/// left is a genuine path read (the only operand `??` accepts).
#[test]
fn cross_resource_identity_coalesce_is_flagged() {
    let root = temp_project("program-coalesce-id-cross", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             \x20   index byTitle(title) unique\n\
             resource Magazine\n\
             \x20   required title: string\n\
             store ^magazines(id: int): Magazine\n\
             fn f(m: Id(^magazines)): Id(^magazines)\n\
             \x20   return ^books.byTitle(\"a\") ?? m\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A unary operator on an identity-typed value is operator misuse: no unary op
/// applies to an identity, so `-b` over a `Id(^books)` is `check.operator_type`, not
/// a silent `Unknown`.
#[test]
fn unary_on_identity_is_an_operator_type_error() {
    let root = temp_project("program-unary-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): bool\n\
                 \x20   return not b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

// --- Non-equality binary operators over concrete non-scalar operands ---
//
// `==`/`!=` over identities, records, and sequences is decided before the scalar
// gate. The other binary operators (`+`, `<`, `and`, `_`, …) shared that gate but
// dropped a concrete non-scalar operand to `Unknown` with no diagnostic. Each
// non-scalar operand is operator misuse, like the unary and `Error` cases.

/// Adding a scalar to an identity (`b + 1` where `b: Id(^books)`) is operator misuse:
/// arithmetic does not apply to an identity, so it is `check.operator_type`, not a
/// silent `Unknown`.
#[test]
fn arithmetic_with_identity_operand_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-arith", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): int\n\
                 \x20   return b + 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Ordering two identities (`b < c`) is operator misuse: comparison ordering does
/// not apply to identities, so it is `check.operator_type`.
#[test]
fn ordering_two_identities_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-order", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books), c: Id(^books)): bool\n\
                 \x20   return b < c\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A logical operator over an identity operand (`b and true`) is operator misuse:
/// `and` requires `bool`, so an identity operand is `check.operator_type`.
#[test]
fn logical_with_identity_operand_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-and", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): bool\n\
                 \x20   return b and true\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// Concatenating a string with an identity (`"a" _ b`) is operator misuse: `_`
/// joins two strings, so an identity operand is `check.operator_type`.
#[test]
fn concat_with_identity_operand_is_an_operator_type_error() {
    let root = temp_project("program-bin-id-concat", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books)): string\n\
                 \x20   return \"a\" _ b\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A scalar-only binary operation (`1 + 2`) checks clean.
#[test]
fn scalar_arithmetic_still_checks_clean() {
    let root = temp_project("program-bin-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(): int\n\
                 \x20   return 1 + 2\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

// --- `if`/`while` conditions over concrete non-scalar types ---
//
// A condition must be `bool`. A condition whose type is a concrete non-scalar (an
// identity, a whole record, a sequence) cannot be `bool`, so it is flagged like a
// wrong scalar or an `Error` condition, never swallowed.

/// `if b` over an identity condition is `check.condition_type` — an identity is not
/// `bool`.
#[test]
fn if_identity_condition_is_a_condition_type_error() {
    let root = temp_project("program-if-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books))\n\
                 \x20   if b\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// `while b` over an identity condition is `check.condition_type` — the `while`
/// condition is checked the same way as `if`.
#[test]
fn while_identity_condition_is_a_condition_type_error() {
    let root = temp_project("program-while-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Id(^books))\n\
                 \x20   while b\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// `if b` over a whole-record condition is `check.condition_type` — a record is not
/// `bool`.
#[test]
fn if_whole_record_condition_is_a_condition_type_error() {
    let root = temp_project("program-if-record", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(b: Book)\n\
                 \x20   if b\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// `if xs` over a sequence condition is `check.condition_type` — a sequence is not
/// `bool`.
#[test]
fn if_sequence_condition_is_a_condition_type_error() {
    let root = temp_project("program-if-seq", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(xs: sequence[int])\n\
                 \x20   if xs\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.condition_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A genuine `bool` condition (`if s == "x"`) checks clean.
#[test]
fn bool_condition_still_checks_clean() {
    let root = temp_project("program-if-bool", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            &format!(
                "{OPERATOR_OPERANDS}\
                 fn f(s: string)\n\
                 \x20   if s == \"x\"\n\
                 \x20       var x = 1\n"
            ),
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}

// --- `??` over a mixed scalar / non-scalar pair ---
//
// `??` defaults a path read with a value of the leaf's type. A pair where one side
// is a concrete non-scalar and the other is a scalar is a category error, not a
// silently-accepted default: the scalar fallback would drop the non-scalar to
// `Unknown` and pass it through. `type_compatible` drives the verdict.

/// A string-leaf read defaulted with an identity (`book.title ?? id`) is a category
/// error reported as `check.operator_type`: a `Id(^books)` cannot default a `string`
/// leaf.
#[test]
fn string_leaf_coalesced_with_identity_is_flagged() {
    let root = temp_project("program-coalesce-str-id", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn f(id: Id(^books)): string\n\
             \x20   return ^books(1).title ?? id\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A whole-record read defaulted with a scalar (`^books(1) ?? 1`) is a category
/// error reported as `check.operator_type`: a scalar cannot default a whole record.
#[test]
fn whole_record_coalesced_with_scalar_is_flagged() {
    let root = temp_project("program-coalesce-record-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn f(): Book\n\
             \x20   return ^books(1) ?? 1\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "check.operator_type"),
        "{:#?}",
        report.diagnostics
    );
}

/// A scalar leaf defaulted with a matching scalar (`book.title ?? "x"`) checks clean.
#[test]
fn scalar_coalesce_still_checks_clean() {
    let root = temp_project("program-coalesce-scalar", |root| {
        write(
            root,
            "src/shelf/ops.mw",
            "module shelf::ops\n\
             resource Book\n\
             \x20   required title: string\n\
             store ^books(id: int): Book\n\
             fn f(): string\n\
             \x20   return ^books(1).title ?? \"x\"\n",
        );
    });
    let (report, _) = check_project(&root, &config()).expect("check");
    assert!(!report.has_errors(), "{:#?}", report.diagnostics);
}
