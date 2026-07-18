# Control Flow

Marrow control flow consists of block statements. Conditions are `bool`;
assignment and branching are not expressions.

## Conditional Statements

`if` selects a branch:

```text
if condition {
    statements
} else if other {
    statements
} else {
    statements
}
```

`if const` evaluates an optional expression once. The then branch receives a
bare binding when the result is present; the else branch runs for `absent`.

```mw
module docs::conditional

fn maybeName(enabled: bool): string? {
    if enabled { return "Marrow" }
    return absent
}

pub fn display(enabled: bool) {
    if const name = maybeName(enabled) {
        print(name)
    } else {
        print("(none)")
    }
}
```

The subject may be any `T?`, including a local optional, a local collection
read, a durable read, a user-function result, or an optional standard-library
result.

An `if const` head may chain several bindings and an optional trailing condition
with `and`. Each subject is evaluated left to right and only when every earlier
subject was present, each binding is in scope for the later subjects and the
then branch, and the else branch runs when any subject is absent or the trailing
condition is false.

```mw
module docs::conditional_chain

fn maybeName(id: int): string? {
    if id > 0 { return "Marrow" }
    return absent
}

fn maybeAge(id: int): int? {
    if id > 0 { return 21 }
    return absent
}

pub fn describe(id: int): string {
    if const name = maybeName(id) and const age = maybeAge(id) and age >= 18 {
        return name
    }
    return "unknown"
}
```

## Let-else Bindings

A `const` or `var` binding may take an `else` block that runs when the subject
is absent. The else block must diverge — it cannot fall through — so past the
binding the name is always in scope with the present value.

```mw
module docs::let_else

fn maybeName(id: int): string? {
    if id > 0 { return "Marrow" }
    return absent
}

pub fn greet(id: int): string {
    const name = maybeName(id) else {
        return "unknown"
    }
    return name
}
```

## Boolean Evaluation

Expressions evaluate operands and call arguments from left to right. `and`
evaluates its right operand only when the left is true. `or` evaluates its right
operand only when the left is false. `optional ?? fallback` evaluates the
fallback only when the left value is absent.

These rules apply even when the skipped expression would call a function or
raise an error.

## `while`

```text
while condition {
    statements
}
```

The condition is evaluated before every iteration. `while` has no iteration
limit and may run indefinitely when the program does not make progress.

## `for`

`for` traverses an integer range, a local collection, or a durable root or
branch family. A range head binds one name over the integers a range covers (see
[Ranges](traversal-and-indexes.md#ranges)); a temporal range is not current
behavior.

```text
for name [, name ...] in [reversed] iterable [by step] {
    statements
}
```

A durable traversal is always bounded and takes a different head — `for k in
<place> at most N [from f]` with a mandatory `on more` block. The forms, binding
arity, and frozen-set behavior are defined in
[Traversal and indexes](traversal-and-indexes.md). Loop variables are immutable
and scoped to the loop body.

## Loop Exits

`continue` begins the next iteration of the innermost loop. `break` exits the
innermost loop. Neither form accepts a label or value. `return` exits the
current function, so a helper function is the direct way to leave several
nested loops with a result.

```mw
module docs::nested_exit

resource Book {
    required title: string

    notes[pos: int] {
        required text: string
    }
}

store ^books[id: int]: Book

fn hasNote(wanted: string): bool {
    for id in ^books at most 100 {
        for pos in ^books[id].notes at most 100 {
            if const note = ^books[id].notes[pos] {
                if note.text == wanted { return true }
            }
        } on more return false
    } on more return false
    return false
}

pub fn show(wanted: string) {
    if hasNote(wanted) {
        print("found")
    }
}
```

When these exits leave a transaction normally, the transaction commits before
control transfers. An escaping error instead rolls it back.

## `unreachable`

`unreachable("static text")` asserts that control never reaches this point. It is
the sole application-declared invariant fault. It takes exactly one static string
literal — never a computed value — recording the invariant the author believes
holds. Reaching it raises the source-uncatchable `run.unreachable` fault, mapped to
the statement, carrying that text.

`unreachable` diverges: control never continues past it. It therefore stands as the
final statement of a value-returning function whose earlier branches already cover
every real case, without a spurious "not all paths return a value" error.

```mw
module docs::invariant

pub fn sign(n: int): int {
    if n > 0 { return 1 }
    if n < 0 { return -1 }
    if n == 0 { return 0 }
    unreachable("every int is positive, negative, or zero")
}
```

It is a statement, not an expression: it cannot be used where a value is required.

## Checked Arithmetic

By default an integer operation that overflows or divides by zero raises a
source-uncatchable runtime fault (`run.overflow`, `run.divide_by_zero`). The
adjacent `checked` form instead handles those faults locally with diverging arms.
It wraps exactly one integer operation — `+`, `-`, `*`, `/`, `%`, or negation — and
binds the result of the non-faulting path to a `const`/`var` or returns it:

```mw
module docs::checked_arithmetic

pub fn safeDivide(a: int, b: int): int {
    return checked a / b
        on out_of_range {
            return -1
        } on zero_divisor return 0
}

pub fn product(a: int, b: int): int? {
    const p: int = checked a * b
        on out_of_range return absent
    return p
}
```

Each `on` arm runs when the operation faults that way and must diverge — every path
through it must `return`, `break`, `continue`, or reach `unreachable`, so control
never falls out of an arm back into the surrounding code. The required arms
are exactly the faults the operation can raise: `+`, `-`, `*`, and negation take an
`on out_of_range` arm; `/` and `%` take both `on out_of_range` (for the
`i64::MIN / -1` case) and `on zero_divisor`. A missing, extra, or non-diverging arm
is a compile error.

## Enum Matching

A closed enum is a nominal set of declared members. A member may be bare or
carry a dense typed payload written as `name(field: Type, ...)`; a payload field
type is any scalar (through an alias if desired):

```mw
module docs::matching

enum Shape {
    dot
    circle(radius: int)
    rect(width: int, height: int)
}

fn area(s: Shape): int {
    match s {
        dot => return 0
        circle(r) => return r * r
        rect(w, h) => return w * h
    }
}

pub fn describe(s: Shape): int {
    return area(s)
}
```

An enum value is constructed as `Enum::member` for a bare member and
`Enum::member(field: value, ...)` for a payload member (arguments are named). A
`match` dispatches on the scrutinee's member. Each arm names one member relative
to the scrutinee enum; a payload member's arm binds its payload positionally
(`circle(r)`) or omits the bindings to ignore the payload (`circle`). A `match`
must cover every member exactly once and has no wildcard arm; a missing member
is `check.match_nonexhaustive` and a malformed arm is `check.match_arm`.

`==` and `!=` are exact enum equality — the same member with equal payload:

```mw
module docs::equality

enum Color {
    red
    green
}

fn same(a: Color, b: Color): bool {
    return a == b
}
```

Hierarchical enums — `category` members that group descendants, qualified arms
such as `cat::tiger`, and the `is` subtree-membership operator — are a future
direction (see [`future/general-purpose-language.md`](../future/general-purpose-language.md)).
On the current line every member is a selectable leaf and the checker rejects a
`category` or nested member as `check.unsupported`.

## Prefix `try` And `transaction`

Prefix `try <expr>` propagates a `Result<T, E>` failure. It is written as the
top-level right-hand side of a statement — `const x = try f()`, `var x = try
f()`, `return try f()`, or a bare `try f()` — never nested inside a larger
expression. It evaluates `expr` to a `Result<T, E>`: an `ok(v)` yields the value
`v`, and an `err(e)` returns `err(e)` immediately from the enclosing function. The
enclosing function must return `Result<U, E>` with the same error type `E`; there
is no implicit error conversion.

```mw
module docs::propagation

fn checkPort(n: int): Result<int, string> {
    if n < 0 { return err("negative port") }
    return ok(n)
}

pub fn openTwice(a: int, b: int): Result<int, string> {
    const x = try checkPort(a)
    const y = try checkPort(b)
    return ok(x + y)
}
```

`Result<T, E>` and `Option<T>` are ordinary value types; see
[Types and values](types-and-values.md). `transaction` groups durable effects;
its exit rules are defined in
[Errors and transactions](errors-and-transactions.md).
