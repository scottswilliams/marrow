# Source and syntax

A Marrow program is UTF-8 text in `.mw` files. Braces delimit blocks, a line
break ends a statement, and `^` marks a durable place.

## Files and modules

A file under `src/` is a module named by its path. `src/docs/syntax/file.mw`
begins with `module docs::syntax::file`:

```mw
module docs::syntax::file

// A shelf keyed by an integer id.
resource Book {
    required title: string
    shelf: string
}

store ^books[id: int]: Book

const defaultShelf = "unsorted"

/// The shelf a book sits on, or the default.
pub fn shelfOf(id: int): string {
    return ^books[id].shelf ?? defaultShelf
}

test "an unshelved book" {
    ^books[1].title = "Small Gods"
    assert shelfOf(1) == "unsorted"
}
```

The header comes first, then declarations in any order. `resource` and `store`
describe durable data, `const` names a value, and `pub fn` exports a function.
`^books[id].shelf` reads one field of one entry; the read is optional because
either may be absent, and `??` supplies the default. The `test` block runs
against a fresh in-memory store, and a durable write in a test body is a bare
statement ([tests](tests.md)). The module rules are in
[modules and functions](modules-and-functions.md).

## Comments

`//` starts a comment that runs to the end of the line. `///` starts a
documentation comment and precedes a declaration, member, or parameter. A tab
is an error anywhere in a file, including inside a string or a comment.
Indentation carries no meaning; the formatter writes four spaces.

## Names and keywords

A name begins with an ASCII letter or `_` and continues with ASCII letters,
digits, or `_`. Names are case-sensitive. The reserved words are listed in
[machine-readable language facts](../tools/ai-legibility.md#reserved-words).
`by`, `at most`, `from`, `on more`, and the duration units such as
`days` are read as keywords only in their own positions; elsewhere they are
ordinary names. `catch` and `throw` are not keywords; statement-head forms from
the removed exception channel report `parse.syntax`.

## Literals

| Kind | Examples | Notes |
|---|---|---|
| Integer | `0`, `-12`, `1000` | Signed decimal |
| Decimal (**future**) | `12.50` | Parses; reports `check.unsupported` |
| Boolean | `true`, `false` | |
| String | `"text"`, `"line\n"` | UTF-8 text with escapes |
| Interpolated string | `$"id: {id}"` | One expression per `{...}` hole |
| Duration | `3 days`, `10 minutes`, `duration("PT600S")` | Units: `second`, `minute`, `hour`, `day`, `week` |
| Date | `date("2026-03-01")` | One canonical form |
| Instant | `instant("2026-03-01T09:00:00Z")` | One canonical form |
| Absence | `absent` | The absent case of an optional |

```mw
module docs::syntax::literals

test "literal forms" {
    const count = 12
    const title = "Small Gods"
    const due = date("2026-03-01")
    const grace = 3 days
    assert $"{title}: {count}" == "Small Gods: 12"
    assert string(grace) == "PT259200S"
    assert addDays(due, 3) == date("2026-03-04")
}
```

`3 days` folds to a duration at compile time. A month or a year has no fixed
length and is not a unit.

String escapes are `\\`, `\"`, `\n`, `\r`, `\t`, and `\u{H}` with one to six
hexadecimal digits naming a Unicode scalar value. Any other character may appear
directly. Inside `$"..."`, `{{` and `}}` are literal braces and `\u{H}` stays an
escape. A hole holds a scalar, an enum value, or an entry identity, rendered as
`string(...)` renders it; `Option` and `Result` values render as
`Option::some(1)` and `Result::err(E::bad)`. A struct, list, map, or optional in a
hole is a `check.unsupported` error.

`bytes("Marrow")` constructs the UTF-8 bytes of a string. **Future:** The parser
recognizes direct byte-literal spelling such as `b"Marrow"`; today it reports
`check.unsupported`. [Types and values](types-and-values.md) defines each value.

## Blocks and lines

A header line ends with `{`, the block closes with `}` on its own line, and a
trailing clause cuddles the closing brace: `} else {`, `} else if c {`,
`} on more {`. Every block takes braces, including a single statement. There is
no statement separator, so a `;` is a syntax error.

A line break ends a statement. A logical line continues across a physical line
break in exactly two cases: while inside an open `(` or `[`, and after a
trailing `and`, `or`, `,`, or `=`. There is no continuation character; any other
break ends the statement.

```mw
module docs::syntax::lines

fn describe(title: string, author: string, shelf: string): string {
    return $"{title} by {author} on {shelf}"
}

test "a call spans lines" {
    const text = describe(
        "Small Gods",
        "Terry Pratchett",
        "fantasy",
    )
    assert text == "Small Gods by Terry Pratchett on fantasy"
}
```

The open `(` carries the call across four lines, and a trailing comma is
accepted. This is the one layout the formatter keeps; `marrow fmt` puts a
condition broken after `and` back onto one line.

## Bindings and assignment

`const` creates a binding that cannot be reassigned. `var` creates a mutable
binding. A top-level `const` holds one scalar literal. A local `const` is
evaluated when control reaches it. Reassigning a `const` is a `check.type`
error.

Assignment is a statement. The forms are `place = expression` and the compound
`+=`, `-=`, `*=`, `/=`, and `%=`. Equality uses `==`; `=` is never equality.
Only a `var` binding or an assignable member, collection, or durable place
appears on the left.

## Expressions and operators

Primary expressions are literals, names, paths, calls, constructors, and
parenthesized expressions. Postfix forms are member access `.`, key access
`[...]`, a call `(...)`, and optional member access `?.`. Operators bind from
tighter to looser in this order:

```text
-value  not value
*  /  %
+  -
optional ?? fallback
..  ..=
<  <=  >  >=  in  not in
==  !=
and
or
```

```mw
module docs::syntax::expressions

struct Loan {
    days: int
}

test "operators" {
    const open: Loan? = Loan(days: 10)
    const closed: Loan? = absent
    assert open?.days ?? 0 > 3
    assert closed?.days ?? 0 == 0
    assert 2 + 3 * 4 == 14
    assert 5 in 1..=5 and 6 not in 1..=5
}
```

`?.` reads a member through an optional value: `absent` when the value is
absent, the member as an optional otherwise. `??` binds tighter than a
comparison, so `open?.days ?? 0 > 3` compares the fallback result. `not` is
unary, so negated membership is spelled `6 not in 1..=5`. The operand rules and
division by zero are in [types and values](types-and-values.md#operators).
Project and generic functions take positional arguments. A struct constructor
names its fields: `Loan(days: 10)`.

## Paths

A local path begins with a binding. A durable path begins with `^` and a store
name. Both use `.` for a member and `[...]` for a key:

```text
book.title
shelves["fantasy"].count
^books[id].title
^books[id].notes[pos].text
```

Keys are positional: `^grid[a, b]`. An `Id(^books)` may stand directly in
`^books[id]`. [Durable places](durable-places.md) defines what a durable path
reads and writes.

## Declarations and statements

A file declares `module`, `use`, `const`, `fn` and `pub fn`, `alias`, `type`,
`struct`, `enum`, `resource`, `store` with its indexes, and `test`. An absent module reports
`check.import` and an absent function reports `check.type`; a cross-module call
to a non-public function reports `check.visibility`. Each form is defined by
[modules and functions](modules-and-functions.md), [types and values](types-and-values.md),
[resources](resources.md), [durable places](durable-places.md),
[traversal and indexes](traversal-and-indexes.md), or [tests](tests.md).

A statement is a `const` or `var` binding, an assignment, an expression, `if`
and `if const`, `while`, `for`, `match`, `break`, `continue`, `return`,
`require`, prefix `try`, `place`, `transaction`, `delete`, `unset`, and
`assert`. A binding may take a let-else tail or a `checked` arithmetic form.
The control statements are defined in [control flow](control-flow.md),
`transaction` and `try` in
[errors and transactions](errors-and-transactions.md), and `place` and `delete`
in [durable places](durable-places.md#named-places).

## Diagnostics

A syntax error is `parse.syntax`, reported with a 1-based line and column:

```text
src/main.mw:4:16: parse.syntax: unexpected character `;`
```

A file that is not valid UTF-8 reports one diagnostic at its first position.
Source nested deeper than 256 levels is `check.nesting_limit`. The nesting bound
and the diagnostic ceilings are in [execution limits](execution-limits.md#limits).
