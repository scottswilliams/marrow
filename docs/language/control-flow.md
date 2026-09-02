# Control flow

Marrow control flow consists of block statements. Conditions are `bool`;
assignment and branching are not expressions.

A function that branches, loops, and returns:

```mw
module docs::control::opening

fn label(n: int): string? {
    if n > 0 {
        return "positive"
    }
    return absent
}

pub fn firstLabel(limit: int): string {
    for n in -1..limit {
        if const word = label(n) {
            return word
        }
    }
    return "none"
}

test "firstLabel finds the first positive number" {
    assert firstLabel(3) == "positive"
    assert firstLabel(1) == "none"
}
```

`if n > 0` tests a `bool`. `for n in -1..limit` walks the integers from `-1` up
to `limit`, excluded. `if const word = label(n)` runs its block only when the
optional result is present, with `word` bound to the value. `return` leaves the
function with a value, from inside the loop or after it.

## Evaluation order

Expressions evaluate operands and call arguments from left to right. `and`
evaluates its right operand only when the left is true. `or` evaluates its right
operand only when the left is false. `optional ?? fallback` evaluates the
fallback only when the left value is absent. A skipped operand is skipped even
when it would call a function or raise a fault.

## Conditionals

`if condition { ... }` runs its block when the condition is true. `else if`
and `else` clauses follow the block, and the first true condition wins.

`if const name = subject { ... }` evaluates an optional subject once. The block
runs when the subject is present, with `name` bound to the value; the else
branch runs for `absent`. The subject is any `T?`: a local optional, a
collection read, a [durable read](durable-places.md#reading), or a function
result. `else if` may follow an `if const` block.

An `if const` head chains several bindings, and a trailing condition, with
`and`:

```mw
module docs::control::chain

pub fn stock(prices: Map<string, int>, counts: Map<string, int>): int {
    if const price = prices["pen"] and const n = counts["pen"] and n > 0 {
        return price * n
    }
    return 0
}

test "stock needs both bindings and the condition" {
    var prices: Map<string, int> = Map()
    var counts: Map<string, int> = Map()
    prices["pen"] = 3
    assert stock(prices, counts) == 0
    counts["pen"] = 4
    assert stock(prices, counts) == 12
}
```

Subjects are evaluated left to right, and evaluation stops at the first absent
one. Each binding is in scope for the subjects after it and for the block. The
else branch runs when any subject is absent or the trailing condition is false.

## Let-else bindings

A `const` or `var` binding may take an `else` block that runs when the subject
is absent. The else block diverges: every path through it returns or reaches
`unreachable`. Past the binding the name is in scope with the present value.

```mw
module docs::control::let_else

pub fn priceOf(prices: Map<string, int>, item: string): string {
    const price = prices[item] else {
        return "no price"
    }
    return string(price)
}

test "priceOf proves the price present" {
    var prices: Map<string, int> = Map()
    prices["pen"] = 3
    assert priceOf(prices, "pen") == "3"
    assert priceOf(prices, "ink") == "no price"
}
```

`priceOf` proves `price` present in one statement. The else block ends the
function, so the `return string(price)` line sees an `int`.

## While

`while` repeats a block as long as a condition holds:

```mw
module docs::control::while_loop

pub fn digits(n: int): int {
    var rest = n
    var count = 1
    while rest >= 10 {
        rest /= 10
        count += 1
    }
    return count
}

test "digits counts decimal digits" {
    assert digits(7) == 1
    assert digits(1234) == 4
}
```

The condition is evaluated before every iteration. `while` has no bound of its
own. A loop that does not terminate exhausts the invocation's instruction budget
and stops with `run.budget` ([execution limits](execution-limits.md#limits)).

## For

`for` walks an integer range, a list, a map, or a durable place:

```mw
module docs::control::loops

pub fn total(prices: Map<string, int>): int {
    var sum = 0
    for name, price in prices {
        sum += price
    }
    return sum
}

pub fn sumOddBelow(limit: int, stop: int): int {
    var sum = 0
    for n in 1..=limit {
        if n % 2 == 0 {
            continue
        }
        if n >= stop {
            break
        }
        sum += n
    }
    return sum
}

test "for walks a map and a range" {
    var prices: Map<string, int> = Map()
    prices["pen"] = 2
    prices["ink"] = 5
    assert total(prices) == 7
    assert sumOddBelow(10, 7) == 9
}
```

A range binds one name and may take `by step`
([ranges](traversal-and-indexes.md#ranges)). A list binds one name to each
element in order. A map binds one name to each key, or two names to each key and
value, in ascending key order. A loop variable is a `const` scoped to the loop
body.

A durable `for` states its bound:
`for id in ^books at most 100 { ... } on more { ... }`.
[Traversal and indexes](traversal-and-indexes.md#bounded-durable-traversal)
defines it.

## Loop exits

`continue` begins the next iteration of the innermost loop. `break` exits the
innermost loop. Neither form takes a label or a value. `sumOddBelow` above
skips even numbers with `continue` and stops at `stop` with `break`; the sum is
`1 + 3 + 5`. `return` exits the whole function, so a helper function is the
direct way to leave several nested loops with a result.

A `return` inside a [transaction](errors-and-transactions.md#transactions)
block commits the block before it returns. `break` and `continue` cannot leave
a `transaction` block; a loop written inside the block may use them freely.

## Divergence

`unreachable("static text")` states that control does not reach this point.
Reaching it stops the program with `run.unreachable`, carrying the text. It
takes one static string literal and diverges, so it can stand as the final
statement of a value-returning function.

```mw
module docs::control::invariant

pub fn sign(n: int): int {
    if n > 0 {
        return 1
    }
    if n < 0 {
        return -1
    }
    if n == 0 {
        return 0
    }
    unreachable("every int is positive, negative, or zero")
}

test "sign covers every int" {
    assert sign(9) == 1
    assert sign(-9) == -1
    assert sign(0) == 0
}
```

The three `if` blocks cover every `int`, but the compiler does not reason about
that. `unreachable` closes the function without a fourth return.

`todo("static text")` marks a path that is not written yet. It has the form and
rules of `unreachable`; reaching it stops the program with `run.todo` instead.
Both are statements; neither stands where a value is required.

## Checked arithmetic

An integer operation that overflows or divides by zero stops the program with
`run.overflow` or `run.divide_by_zero`. `checked` handles those two faults at
the operation. It wraps one `+`, `-`, `*`, `/`, `%`, or negation and binds or
returns the result of the successful path:

```mw
module docs::control::checked_arithmetic

pub fn safeDivide(a: int, b: int): int {
    return checked a / b
        on out_of_range {
            return -1
        } on zero_divisor {
            return 0
        }
}

pub fn product(a: int, b: int): int? {
    const p: int = checked a * b
        on out_of_range {
            return absent
        }
    return p
}

test "checked arms replace the fault" {
    assert safeDivide(7, 2) == 3
    assert safeDivide(7, 0) == 0
    assert safeDivide(minInt, -1) == -1
    assert product(3, 4) ?? 0 == 12
    assert product(maxInt, 2) ?? 0 == 0
}
```

Each `on` arm runs when the operation faults that way and diverges: every path
through it returns, breaks, continues, or reaches `unreachable`. The arms are
exactly the faults the operation can raise; a missing, extra, or non-diverging
arm is a compile error.

| Operation | Arms |
|---|---|
| `+`, `-`, `*`, `-x` | `on out_of_range` |
| `/`, `%` | `on out_of_range` and `on zero_divisor` |
| `/`, `%` with a nonzero literal divisor | `on out_of_range` |

`minInt / -1` overflows, so a division keeps `on out_of_range` whatever the
divisor. `checked x / 100` has no `on zero_divisor` arm.

## Match

`match` dispatches on the member of an [enum](types-and-values.md#enums)
value:

```mw
module docs::control::matching

enum Shape {
    dot
    circle(radius: int)
    rect(width: int, height: int)
}

pub fn area(s: Shape): int {
    match s {
        dot => {
            return 0
        }
        circle(r) => {
            return r * r
        }
        rect(w, h) => {
            return w * h
        }
    }
}

test "area matches every member" {
    assert area(Shape::dot) == 0
    assert area(Shape::circle(radius: 3)) == 9
    assert area(Shape::rect(width: 2, height: 5)) == 10
}
```

Each arm names one member of the enum, bare. A payload member's arm binds its
payload positionally (`circle(r)`) or omits the bindings to ignore the payload
(`circle`). A `match` covers every member exactly once and has no wildcard arm.
A missing member is `check.match_nonexhaustive`; a malformed arm is
`check.match_arm`.

`Option` and `Result` are enums and match the same way, with arms `some(v)`
and `none`, or `ok(v)` and `err(e)`.

## Require guards

`require condition else value` returns a failure unless a boolean precondition
holds. It means the same as `if not condition { return err(value) }`. The
function returns `Result<T, E>`, and `value` is an `E`. The value is evaluated
only when the condition is false.

```mw
module docs::control::require_guard

fn isReserved(name: string): bool {
    return name == "admin"
}

pub fn admitName(name: string): Result<string, string> {
    require not isEmpty(name) else "name is empty"
    require not isReserved(name) else "name is reserved"
    return ok(name)
}

test "admitName rejects empty and reserved names" {
    const admitted: Result<string, string> = ok("ada")
    const empty: Result<string, string> = err("name is empty")
    assert admitName("ada") == admitted
    assert admitName("") == empty
}
```

The two guards form a prelude: each states one condition and its failure, and
the body below them runs with both established. `require` originates a
failure; `try` propagates one. The [guard prelude](idioms.md#guard-prelude)
shows the guard forms together.

A `require` cannot stand inside a `transaction` block that its own function
owns, because its failure exit would leave the block without a commit
([guards inside a block](errors-and-transactions.md#guards-inside-a-block)).

## Prefix try

`try expr` propagates a `Result<T, E>` failure. It is the whole right-hand side
of a statement: `const x = try f()`, `var x = try f()`, `return try f()`, or a
bare `try f()`. An `ok(v)` yields `v`; an `err(e)` returns `err(e)` from the
enclosing function at once. The enclosing function returns `Result<U, E>` with
the same error type `E`.

```mw
module docs::control::propagation

fn checkPort(n: int): Result<int, string> {
    if n < 0 {
        return err("negative port")
    }
    return ok(n)
}

pub fn openTwice(a: int, b: int): Result<int, string> {
    const x = try checkPort(a)
    const y = try checkPort(b)
    return ok(x + y)
}

test "openTwice propagates the first failure" {
    const opened: Result<int, string> = ok(523)
    const failed: Result<int, string> = err("negative port")
    assert openTwice(80, 443) == opened
    assert openTwice(80, -1) == failed
}
```

The second `try` does not run when the first fails: `openTwice(-1, 80)` returns
`err("negative port")` after one call.

Inside a `transaction` block, `try` keeps this meaning. Its failure exit
carries no commit, so a `try` cannot stand inside a block that its own function
owns ([guards inside a block](errors-and-transactions.md#guards-inside-a-block)).
