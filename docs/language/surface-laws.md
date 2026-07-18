# The surface laws

This page states lexical properties that hold across the whole current `.mw`
language: which marks carry consequence, which text patterns are guaranteed to be
complete inventories of a construct, and two design laws that keep those
guarantees stable. The other reference pages define the constructs named here;
this page describes the surface they share.

## The Closed Sigil Economy

A mark carries consequence, and everything else is words. The marked set is
closed: the following marks are the entire vocabulary that signals an act with a
result beyond plain local computation. Nothing decorative joins the set, and
nothing in it is ever implied.

| Mark | Marks |
|---|---|
| `^` | a durable place — a location whose value outlives the program (see [Durable places](durable-places.md)) |
| `place` | a binding that names one concrete durable entry address (see [Named places](durable-places.md#named-places)) |
| `transaction` | the block whose durable changes commit or roll back as a unit (see [Errors and transactions](errors-and-transactions.md#transactions)) |
| `at most` / `on more` | a bounded durable traversal and the arm that runs when more keys remain (see [Traversal](traversal-and-indexes.md#bounded-durable-traversal)) |
| `checked` / `on` | fault-armed integer arithmetic and its diverging fault arms (see [Checked arithmetic](control-flow.md#checked-arithmetic)) |
| `try` | propagation of a `Result<T, E>` failure out of the enclosing function (see [Prefix `try`](control-flow.md#prefix-try-and-transaction)) |
| `delete` | removal of a durable place (see [Deletion](durable-places.md#deletion)) |
| `$` | the opener of an interpolated string, `$"…"` (see [Literals](source-and-syntax.md#literals)) |

Ordinary logic is carried by words, not punctuation: `and`, `or`, `not`,
`exists`, `is`, and `in` read as text and mean what they say, while brackets,
braces, and parentheses carry only structure. Plain arithmetic wears no mark at
all — its plainness is a checked fact rather than a convention, because a function
that touches no durable place is reported as such by the compiler (see
[Access demand](durable-places.md#access-demand)).

In the following module the arithmetic in `perShare` is unmarked except at its
two named fault exits, and the failure that `shareOfDouble` forwards is marked by
`try`. No other point in either function carries a mark, because no other point
has a consequence to signal.

```mw
module main

fn checkPositive(n: int): Result<int, string> {
    if n < 0 { return err("value is negative") }
    return ok(n)
}

fn perShare(total: int, shares: int): int {
    return checked total / shares
        on out_of_range {
            return 0
        } on zero_divisor return 0
}

pub fn shareOfDouble(total: int, shares: int): Result<int, string> {
    const t = try checkPositive(total)
    return ok(perShare(t + t, shares))
}
```

## The Grep Contract

Because each construct above has exactly one spelling and no synonym, a plain text
search for that spelling returns a *complete* inventory: every occurrence of the
construct appears in the results. The guarantee is one-directional — the pattern
finds every real occurrence and misses none. A pattern is a lexical search over
canonical formatted source, so it may also match the same word inside a comment or
a string literal; those are discounted by reading, and they never hide a real
occurrence.

| Pattern | Complete list of |
|---|---|
| `\^` | every durable-place reference in the module |
| `transaction {` | every transaction region |
| `\btry ` | every failure-propagation point |
| `at most` | every bounded durable traversal (a store root, a keyed branch, or an index scan) |
| `\bwhile\b` | every loop with no iteration limit |
| `\bdelete\b` | every durable deletion |
| `unreachable\(` | every application-declared invariant fault |
| `\bpub ` | every public declaration (an exported function or enum) |

Two rows depend on the durable-access rules and are stated precisely:

- `\^` finds every point at which durable data enters the code. Nothing reaches
  durable data without a `^`: an inline operation names the place directly
  (`^books[id].title`), and a [`place`](durable-places.md#named-places) binding is
  bound from a `^` address before its later operations reuse the name. A read or
  write through a place alias therefore carries no `^` on its own line, but the
  location it names was introduced by a `^` on the binding line.
- `\bdelete\b` finds every durable deletion because deletion of a local value is
  rejected (`check.unsupported`): every deletion that checks removes a durable
  place. The direct form is `delete ^…`; a deletion through a place alias is
  `delete <name>`, where `<name>` was bound from a `^` address.

Two constructs that mark a *bound* pair with a construct that marks its absence:
`at most` heads every durable traversal, which is always bounded, while `\bwhile\b`
is the one loop form with no iteration limit. A `for` head over a local collection
or a numeric range is finite by construction and takes no `at most` bound, so the
unbounded case is exactly `while`.

## The No-Synonym Law

The grep contract holds only while each construct keeps a single spelling. This is
a design law on future work: no later construct may introduce a second spelling
for any row of the table above, and none may make a row's pattern incomplete. The
law binds every future extension. When closures are added, a durable touch inside
a closure body still spells `^`, and a bounded traversal inside one still spells
`at most`; a design that would let durable access or an unbounded traversal appear
under a spelling the table does not find is rejected on that ground.

## The `pure` Marker, Closed

There is no `pure` keyword, effect badge, or colored function type in Marrow
source, and none is planned. A mark signals consequence; marking the side that has
no consequence would invert that rule and attach a mark to plain code. The fact
such a mark would carry — that a function touches no durable place — is already
derived by the compiler from the function's [access demand](durable-places.md#access-demand)
and reported in its output. It is compiler-derived output, spoken by the analysis
that establishes it, not a spelling the author writes. The absence of any durable
mark in a function body is itself the readable statement that the function is
storeless.
