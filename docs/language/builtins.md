# Built-Ins

Built-ins are available without `use`. They cover presence, local collections,
text, durable identities, temporal arithmetic, conversion, and canonical output.

## Presence

`exists(place): bool` reports whether a durable path read that may be absent is
present. Its subject is a durable path, not an arbitrary optional value: an
optional value — a local optional, a local collection read, or a user-function
result — is resolved with `if const`, `??`, or `?.` instead. A true branch
narrows a stable durable path read.

A complete-key read of a `unique` managed index, `exists(^root.uidx[keys])`, is
also a presence subject: it reports whether the index holds a matching entry,
the presence half of the [`if const` lookup](traversal-and-indexes.md#reading-an-index)
without binding the found identity. A non-unique index is scan-only and has no
`exists` probe.

`if const`, `??`, and `?.` are language constructs rather than calls; they
resolve any optional value:

```mw
module docs::builtins_presence

fn maybeNumber(enabled: bool): int? {
    if enabled { return 4 }
    return absent
}

pub fn number(enabled: bool): int {
    if const n = maybeNumber(enabled) { return n }
    return maybeNumber(enabled) ?? 0
}
```

A direct optional-producing call may have effects. When presence resolution
guards a path read, effects cannot be hidden in the read base or key
expressions. Bind an effectful result first, then read a sparse member from the
stable binding.

Presence handling does not suppress a type mismatch, invalid required data, or
decoding failure.

## Local Collection Views

`length(collection): int` reports a `List` or `Map` element count. `isEmpty` also
accepts a `string` (the text floor form below).

Additional collection projections and scalar-length operations are not current
built-ins.

## Text

A small closed set of pure text built-ins is available without `use`. This is the
whole text floor; there is no general string library.

| Form | Result |
|---|---|
| `isEmpty(text): bool` | Whether `text` is the empty string |
| `contains(haystack, needle): bool` | Whether `haystack` contains `needle` as a substring |
| `trim(text): string` | `text` with leading and trailing Unicode whitespace removed |
| `split(text, separator): List<string>` | The substrings of `text` separated by each non-overlapping occurrence of `separator`, in order; an empty separator yields the single-element list `[text]` |
| `lines(text): List<string>` | The lines of `text`, split on line feeds with a carriage return before a line feed removed; a final line terminator adds no trailing empty line |
| `join(parts: List<string>, separator): string` | The `parts` concatenated in order with `separator` between adjacent elements |

`split` and `lines` return a `List<string>`; the result honors the same length and
aggregate-size bounds as any list (see [Execution limits](execution-limits.md)). `join`
honors the string concatenation ceiling.

```mw
module docs::text_floor

pub fn isBlank(s: string): bool {
    return isEmpty(trim(s))
}

pub fn partCount(text: string): int {
    return length(split(text, ","))
}

pub fn rejoin(text: string): string {
    return join(split(text, ","), ";")
}
```

## Collections

The finite collection types `List<T>` and `Map<K, V>` are built through a closed
set of procedural built-ins — there is no method syntax. `List()`/`Map()`
construct an empty collection of the expected type; `append` adds an element and
yields the updated list (collections are values); `length`/`isEmpty` report size.
A collection is read with bracket lookup (`xs[i]`, `m[k]`, each yielding the
presence-typed optional) and a map is written with bracket assignment (`m[k] =
value`). See [Lists and maps](types-and-values.md#lists-and-maps) for the current
operation list, the 1-based list positions, and value semantics.

```mw
module docs::collection_builtins

pub fn size(): int {
    var xs: List<int> = List()
    xs = append(xs, 1)
    xs = append(xs, 2)
    return length(xs)
}
```

`isEmpty` also accepts a `string` (the text floor form above). `length` reports a
collection's element or entry count.

The collection constructors `List` and `Map`, together with the text floor names
`isEmpty`, `contains`, `trim`, `split`, `lines`, and `join`, are reserved: a
`fn`, `const`, parameter, or local binding may not redeclare them, because a bare
use of one of these names always resolves to the built-in. The collection
operations `append` and `length` are deliberately *not* reserved — they are common
verbs — so a same-module function of one of those names is admitted, wins at every
call site in that module, and totally shadows the built-in collection operation
there.

## Positional Append

The `append` above grows a local `List` and yields the updated list. A durable
positional `append` that writes to a keyed scalar leaf (`name[pos: int]: T`) and
returns the written 1-based position is **future**: the current compiler does not
check a keyed scalar leaf, so no durable form of `append` is current.

## Entry Identities

`Id(^root, key...): Id(^root)` constructs an identity from explicit declared
keys without reading the store.

`nextId` and `key` are not current built-ins. A caller supplies an identity
directly as an `Id(^root)`. An application that needs a fresh monotonically
increasing key maintains its own durable counter root and allocates from it in
the same transaction as the create; see
[Counter allocation](idioms.md#counter-allocation).

## Output

`marrow run` renders every admitted export result through the canonical value
renderer. Bytes use `0x`-prefixed lowercase hexadecimal, temporal values use
canonical text, and enums use `Enum::member`. `string(...)` and interpolation
use the same scalar, enum, and identity renderings but reject bare aggregates
and presence optionals.

The current language has no streaming output statement.

## Conversion

Two scalar conversion forms are current:

- `string(value): string` renders a current bare scalar, enum value, or entry
  identity through the canonical rendering owner.
- `bytes(text): bytes` encodes a `string` as UTF-8 bytes.

No conversion is implicit, and no current conversion decodes bytes as text.
[Types and values](types-and-values.md#explicit-conversion) gives the exact
rejected-call boundary.

## Temporal

The temporal types `date`, `instant`, and `duration` are constructed from a
static canonical text literal (`date("2026-07-15")`,
`instant("2026-07-15T17:00:00Z")`, `duration("PT3600S")`), validated at compile
time. Two named functions provide date arithmetic:

```text
addDays(d: date, n: int): date
daysBetween(a: date, b: date): int
```

`addDays` returns the date `n` days after `d`; `daysBetween` returns
the signed number of days from `a` to `b`. A result outside years 0001-9999
faults `run.temporal_overflow`. `duration` sums and differences and `instant`
shifts by a `duration` use the `+`/`-` operators. There is no clock builtin: the
current day or instant is passed in as an argument. See
[Temporal Types](types-and-values.md#temporal-types) for the full contract.

## Errors

A recoverable failure a program handles is an ordinary `Result<T, E>` value (see
[Types and values](types-and-values.md)), returned with `err(...)` and propagated
with prefix `try`. There is no throwable error value and no `throw`/`catch`
channel; the distinct failure kinds are described in
[Errors and transactions](errors-and-transactions.md).

## Deletion

`delete place` is a statement, not a call. It removes the addressed durable value
and descendants under the rules in [Durable places](durable-places.md#deletion).
