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
precondition is a diverging guard, and the guard form follows the subject: a
boolean precondition takes `require`, a presence check takes a `const` binding
with a diverging `else`, and a guard inside a `transaction` region the function
owns is a spelled `if`/`return`, because that return is a commit site and the
commit stays deliberate. The guards carry no nesting, so the body that follows
runs with every checked value already present.

```mw
module clinic::lookup

resource Patient {
    required name: string
    wardCode: string
}

store ^patients[pid: int]: Patient

pub fn wardOf(pid: Id(^patients), wards: Map<string, string>): Result<string, string> {
    require exists(^patients[pid]) else "unknown patient"
    const name = ^patients[pid].name else {
        return err("patient has no name")
    }
    const code = ^patients[pid].wardCode else {
        return err($"{name} has no ward")
    }
    const ward = wards[code] ?? "(unassigned)"
    return ok(ward)
}
```

Local and durable presence follow the same shape. A sparse durable read
(`^patients[pid].name`) and a local optional both take a diverging `else` that binds
the present value past the guard; a map lookup falls back with `??`. The divergence
of an `else` block is required by the language — a let-else `else` cannot fall
through (see [Control flow](control-flow.md#let-else-bindings)) — so past the guard
the name is always in scope with a present value. The boolean guard's `require`
(see [Control flow](control-flow.md#require-guards)) is the same shape without a
binding: one line per precondition, the bare failure value after `else`.

The prelude works the same inside a mutating export's `transaction` region. An
in-region `return` commits the region's staged writes before it returns (see
[Errors and transactions](errors-and-transactions.md#transactions)), so the guards
go at the top of the block and the happy path writes below them, with no accumulator
flag threaded through:

```mw
module clinic::admit

resource Bed {
    required ward: string
    patient: string
}

store ^beds[bid: int]: Bed

pub fn assign(bid: int, who: string): Result<int, string> {
    transaction {
        place slot = ^beds[bid]
        if not exists(slot) {
            return err("no such bed")
        }
        slot.patient = who
    }
    return ok(bid)
}
```

Each guard's `return` is a region exit that commits before it returns; a guard that
returns before staging anything commits an empty region and persists no change.

## The Validation Chain

A domain validator combines the three guard tools: let-else reads the subject and
rejects absence, `require` states each boolean precondition on its own line, and
`try` joins a shared guard helper that several validators reuse. The chain below
is the `updateEncounter` arm of the EMR sample application's `validate()`
(`apps/emr/src/changeset.mw`), reduced to one resource pair. Guard order is
rejection order: the first failing line names the rejection the caller sees.

```mw
module emr::validation

resource Encounter {
    required revision: int
    required patientId: int
    required status: string
}

resource PatientAggregate {
    required revision: int
}

store ^encounters[id: int]: Encounter

store ^patientAggregates[patientId: int]: PatientAggregate

enum Rejection {
    resourceMissing(kind: string, id: int)
    staleRevision(kind: string, id: int, expected: int, actual: int)
    revisionOverflow(kind: string, id: int)
    unsupportedStatus(kind: string, id: int, status: string)
    unsupportedTransition(kind: string, id: int, prior: string, intended: string)
    patientAggregateMissing(patientId: int)
}

fn encounterStatusValid(s: string): bool {
    return s == "planned" or s == "in-progress" or s == "finished"
}

fn encounterTransitionOk(prior: string, intended: string): bool {
    if prior == "finished" {
        return intended == "finished"
    }
    return true
}

fn revisionGuards(kind: string, id: int, actual: int, expected: int): Result<bool, Rejection> {
    require actual == expected else Rejection::staleRevision(kind: kind, id: id, expected: expected, actual: actual)
    require actual < maxInt else Rejection::revisionOverflow(kind: kind, id: id)
    return ok(true)
}

pub fn validateUpdate(id: int, expected: int, s: string): Result<bool, Rejection> {
    const e = ^encounters[id] else {
        return err(Rejection::resourceMissing(kind: "encounter", id: id))
    }
    try revisionGuards("encounter", id, e.revision, expected)
    require encounterStatusValid(s) else Rejection::unsupportedStatus(kind: "encounter", id: id, status: s)
    require encounterTransitionOk(e.status, s) else Rejection::unsupportedTransition(kind: "encounter", id: id, prior: e.status, intended: s)
    require exists(^patientAggregates[e.patientId]) else Rejection::patientAggregateMissing(patientId: e.patientId)
    return ok(true)
}
```

The chain reads as a precondition table, and it greps as one: `\brequire ` lists
every boolean precondition in the module, and `\btry ` every point that forwards
a shared guard's rejection (see
[The grep contract](surface-laws.md#the-grep-contract)). A validator owns no
`transaction` region — the mutating export does — so its `require` and `try`
exits are ordinary control flow into that export's committing in-region `return`.

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
progressively appending text across steps. An interpolation hole renders any
canonically renderable value — a scalar, an enum member, or an entry identity —
through the same rendering `string(...)` and program output use (see
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
        } on zero_divisor {
            return 0
        }
}

pub fn sumCents(a: int, b: int): int? {
    const total: int = checked a + b
        on out_of_range {
            return absent
        }
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

## Counter Allocation

A caller supplies its own entry keys: `nextId` and `key` are not built-ins (see
[Built-ins](builtins.md#entry-identities)). An application that needs a fresh,
monotonically increasing key mints one from a durable counter it owns. The counter
is an ordinary keyed root; a single `name`-keyed root serves every sequence in the
program, one entry per sequence name.

```mw
module docs::allocation

resource Counter {
    required value: int
}

store ^idseq[name: string]: Counter

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn createBook(title: string): int {
    transaction {
        place seq = ^idseq["book"]
        const next = (seq.value ?? 0) + 1
        seq.value = next
        ^books[next] = Book(title: title)
        return next
    }
}

pub fn titleOf(id: int): string? {
    return ^books[id].title
}

pub fn seqValue(): int? {
    return ^idseq["book"].value
}
```

The allocation and the write that consumes it share the export's one
`transaction`, so the increment and the create commit or roll back as a unit: a
key is never advanced without the entry it was minted for, and a rolled-back
create leaves the counter untouched. The read `seq.value ?? 0` supplies the
first-allocation value when the counter entry is absent, so no separate
initialization step is needed. Consistency between the counter and the payload
root — advancing `^idseq["book"]` for each `^books` create — is the program's
responsibility; the language enforces only that the two writes share the
transaction.

The allocated `int` is a bare key. To carry an allocated key as an
[entry identity](types-and-values.md#entry-identity), wrap it with `Id(^root, key)`
and use that value as the key operand:

```mw
module docs::allocation_identity

resource Counter {
    required value: int
}

store ^idseq[name: string]: Counter

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn createBook(title: string): Id(^books) {
    transaction {
        place seq = ^idseq["book"]
        const next = (seq.value ?? 0) + 1
        seq.value = next
        const bid = Id(^books, next)
        ^books[bid] = Book(title: title)
        return bid
    }
}

pub fn titleOf(id: Id(^books)): string? {
    return ^books[id].title
}
```

`Id(^books, next)` constructs the identity from the allocated key without reading
the store, and the returned `Id(^books)` round-trips as the key of a later read.

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
    if not exists(^visits[id]) {
        return 0
    }
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
