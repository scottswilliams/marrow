# Control Flow

Marrow control flow consists of block statements. Conditions are `bool`;
assignment and branching are not expressions.

## Conditional Statements

`if` selects a branch:

```text
if condition
    statements
else if other
    statements
else
    statements
```

`if const` evaluates an optional expression once. The then branch receives a
bare binding when the result is present; the else branch runs for `absent`.

```mw
module docs::conditional

fn maybeName(enabled: bool): string?
    if enabled
        return "Marrow"
    return absent

pub fn display(enabled: bool)
    if const name = maybeName(enabled)
        print(name)
    else
        print("(none)")
```

The subject may be any `T?`, including a local optional, a local collection
read, a durable read, a user-function result, or an optional standard-library
result.

## Boolean Evaluation

Expressions evaluate operands and call arguments from left to right. `and`
evaluates its right operand only when the left is true. `or` evaluates its right
operand only when the left is false. `optional ?? fallback` evaluates the
fallback only when the left value is absent.

These rules apply even when the skipped expression would call a function or
raise an error.

## `while`

```text
while condition
    statements
```

The condition is evaluated before every iteration. `while` has no iteration
limit and may run indefinitely when the program does not make progress.

## `for`

`for` traverses an integer or temporal range, a local collection, or a durable
collection:

```text
for name [, name ...] in [reversed] iterable [by step]
    statements
```

The binding arity and key-first behavior are defined in
[Traversal and indexes](traversal-and-indexes.md). Loop variables are immutable
and scoped to the loop body.

## Loop Exits

`continue` begins the next iteration of the innermost loop. `break` exits the
innermost loop. Neither form accepts a label or value. `return` exits the
current function, so a helper function is the direct way to leave several
nested loops with a result.

```mw
module docs::nested_exit

resource Book
    required title: string
    tags(pos: int): string

store ^books(id: int): Book

fn findWanted(wanted: string): Id(^books)?
    for id in ^books
        for pos, tag in ^books(id).tags
            if tag == wanted
                return id
    return absent

pub fn show(wanted: string)
    if const id = findWanted(wanted)
        print(id)
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

pub fn sign(n: int): int
    if n > 0
        return 1
    if n < 0
        return -1
    if n == 0
        return 0
    unreachable("every int is positive, negative, or zero")
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

pub fn safeDivide(a: int, b: int): int
    return checked a / b
        on out_of_range
            return -1
        on zero_divisor
            return 0

pub fn product(a: int, b: int): int?
    const p: int = checked a * b
        on out_of_range
            return absent
    return p
```

Each `on` arm runs when the operation faults that way and must diverge — every path
through it must `return`, `break`, `continue`, `throw`, or reach `unreachable`, so
control never falls out of an arm back into the surrounding code. The required arms
are exactly the faults the operation can raise: `+`, `-`, `*`, and negation take an
`on out_of_range` arm; `/` and `%` take both `on out_of_range` (for the
`i64::MIN / -1` case) and `on zero_divisor`. A missing, extra, or non-diverging arm
is a compile error.

## Enum Matching

An enum is a nominal set of declared members:

```mw
module docs::matching

pub enum Status
    active
    archived
    banned

fn label(status: Status): string
    match status
        active
            return "active"
        archived
            return "archived"
        banned
            return "banned"

pub fn show(status: Status)
    print(label(status))
```

An arm names a member relative to the scrutinee enum. A `match` must cover every
selectable member exactly once; there is no wildcard arm.

Enum members may form a hierarchy. A parent with children is declared
`category` and is not itself a selectable value:

```text
enum Animal
    category cat
        tiger
        housecat
    dog
```

An arm naming a category covers all selectable descendants. A qualified arm
such as `cat::tiger` selects one descendant. The `is` operator tests membership
in a member subtree:

```text
if animal is Animal::cat
    print("cat")
```

`==` remains exact enum-member equality.

## `try` And `transaction`

`try`/`catch` transfers `Error` values. `transaction` groups durable effects.
Their exit rules are defined together in
[Errors and transactions](errors-and-transactions.md).
