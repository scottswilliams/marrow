# How Marrow Is Written

The other reference pages define what the language accepts. This page describes
how idiomatic Marrow is arranged within those rules: the conventional shape of a
function, a file, and a worked example. These are style conventions, not compiler
rules, except where a section notes an enforced form. Every complete module below
checks and is in formatter-canonical form.

## Named Steps Over Deep Call Nesting

Dataflow is named and vertical. When a computation would nest more than two calls
deep, each stage is bound to a `const` and read by the next line, so the transform
reads from top to bottom rather than inside out.

```mw
module docs::named_steps

pub fn slug(raw: string): string {
    const trimmed = trim(raw)
    const words = split(trimmed, " ")
    const joined = join(words, "-")
    return joined
}

pub fn total(xs: List<int>): int {
    var sum = 0
    for x in xs {
        sum += x
    }
    return sum
}

test "example: total" {
    const xs = List(10, 20, 12)
    assert total(xs) == 42
}
```

`slug` names each stage instead of writing `join(split(trim(raw), " "), "-")`.
Aggregation is an accumulator loop: a running binding updated once per element, as
in `total`. The current language has no `fold`, `map`, or `filter`; those depend on
closures, which are a future addition (see
[`future/general-purpose-language.md`](../future/general-purpose-language.md)). The
accumulator loop is the idiom for reducing a collection to a value.

A list's literal contents are written with the variadic constructor, and later
growth uses `append`: `List(10, 20, 12)` states the three elements a list starts
with, while `append(xs, extra)` adds one more. A map is constructed empty with
`Map()` and filled with `m[k] = v`; a map literal is not yet available.

## The Guard Prelude

A `pub fn` opens with its preconditions, one per line, before the happy path. Each
precondition is a diverging guard: an `if` that returns on failure, or a `const`
binding with a diverging `else`. The guards carry no nesting, so the body that
follows runs with every checked value already present.

```mw
module clinic::lookup

resource Patient {
    required name: string
    wardCode: string
}

store ^patients[pid: int]: Patient

pub fn wardOf(pid: Id(^patients), wards: Map<string, string>): Result<string, string> {
    if not exists(^patients[pid]) { return err("unknown patient") }
    const name = ^patients[pid].name else return err("patient has no name")
    const code = ^patients[pid].wardCode else return err($"{name} has no ward")
    const ward = wards[code] ?? "(unassigned)"
    return ok(ward)
}
```

Local and durable presence follow the same shape. A sparse durable read
(`^patients[pid].name`) and a local optional both take a diverging `else` that binds
the present value past the guard; a map lookup falls back with `??`. The divergence
of an `else` block is required by the language — a let-else `else` cannot fall
through (see [Control flow](control-flow.md#let-else-bindings)) — so past the guard
the name is always in scope with a present value.

The prelude is idiomatic for a read-only export. A mutating export reads and writes
inside its `transaction` block rather than guarding before it; see
[Errors and transactions](errors-and-transactions.md#transactions).

## Interpolation For Multi-Part Text, `+` For Accumulation

A fixed message assembled from parts is written as one interpolated string, so the
literal text and its holes read in place. Text built up across steps — appending
lines to a growing body — uses `+` accumulation into a `var`.

```mw
module docs::interpolation

pub fn label(name: string, open: int): string {
    return $"{name}: {open} open"
}

pub fn report(items: List<string>): string {
    var body = ""
    for item in items {
        const line = "- " + item + "\n"
        body += line
    }
    return body
}

test "example: label" {
    assert label("inbox", 3) == "inbox: 3 open"
}

test "example: report" {
    var xs: List<string> = List()
    xs = append(xs, "a")
    xs = append(xs, "b")
    assert report(xs) == "- a\n- b\n"
}
```

`label` formats one fixed message; interpolation keeps the layout of the result
visible in the source. `report` accumulates: `+` and `+=` build the body up across
iterations. Interpolation is for formatting a fixed, multi-part message; `+` is for
progressively appending text across steps. On the current beta line an interpolation
hole renders a `string`, `int`, or `bool`; other values are converted first (see
[Built-ins](builtins.md#output)).

## `checked` As The Arithmetic Signature

Integer arithmetic that can fault — overflow, or division and remainder by zero —
is written `checked`, and each fault it can raise takes a named diverging arm. The
arm is where the danger is spelled: the operation and its recovery sit together, and
the value that leaves the expression is the non-faulting result.

```mw
module docs::arithmetic

pub fn perDayCents(totalCents: int, days: int): int {
    return checked totalCents / days
        on out_of_range {
            return 0
        } on zero_divisor return 0
}

pub fn sumCents(a: int, b: int): int? {
    const total: int = checked a + b
        on out_of_range return absent
    return total
}
```

The required arms are exactly the faults the operation can raise: `+`, `-`, `*`, and
negation take `on out_of_range`; `/` and `%` take both `on out_of_range` and
`on zero_divisor` (see [Control flow](control-flow.md#checked-arithmetic)). Plain
`a + b` carries no arm because the author has not asked to handle a fault locally;
an unmarked operation that overflows or divides by zero stops with a
source-uncatchable runtime fault instead. Writing `checked` is the visible signal
that arithmetic here is expected to survive a fault, and the arm names which one.

## File Skeleton: Shape, Places, Acts

A module is ordered from what data is to where it lives to what the program does:
resource and type declarations (the shape) come first, then `store` roots (the
places), then functions (the acts). A function's `test` blocks sit next to it.

```mw
module clinic::visits

// shape ---------------------------------------------------------------------

resource Visit {
    required patient: string
    chargeCents: int
}

// places --------------------------------------------------------------------

store ^visits[id: int]: Visit

// acts ----------------------------------------------------------------------

pub fn setCharge(id: Id(^visits), cents: int) {
    transaction {
        if exists(^visits[id]) {
            ^visits[id].chargeCents = cents
        }
    }
}

pub fn chargeOf(id: Id(^visits)): int {
    if not exists(^visits[id]) { return 0 }
    return ^visits[id].chargeCents ?? 0
}

test "example: chargeOf" {
    assert chargeOf(Id(^visits, 1)) == 0
}
```

A reader meets each name before its use: the resource shape, then the durable root
declared over it, then the functions that read and write that root. Keeping a test
beside its function makes the function and its checked behavior one unit to read and
to move.

## The Adjacent Example-Test Convention

A `pub fn`'s worked example is written as a neighboring test titled
`example: <name>`, so the example is checked and executed with the rest of the suite
rather than drifting in prose.

```mw
module docs::example_test

pub fn fieldCount(row: string): int {
    return length(split(row, ","))
}

test "example: fieldCount" {
    assert fieldCount("a,b,c") == 3
}
```

This is a naming convention, not a language mechanism: `marrow test` runs the block
because it is an ordinary [test](tests.md), and nothing extracts examples from
documentation comments or fenced code. The `example:` title marks the test's purpose
for a reader and keeps a function's demonstration adjacent to the function.
