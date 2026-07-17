# Built-Ins

Built-ins are available without `use`. They cover presence, local collection
views, ordered navigation, durable identities, output, and conversion.

## Presence

`exists(optional-or-place): bool` reports whether its subject is present. It
accepts optional values and local or durable place reads that may be absent,
including a user-function result. A definite local scalar is rejected as always
present. A true branch narrows a stable place read, but testing one function
call does not narrow a later call.

`if const`, `??`, and `?.` are language constructs rather than calls, but they
resolve the same optional values:

```mw
module docs::builtins_presence

fn maybeNumber(enabled: bool): int? {
    if enabled { return 4 }
    return absent
}

pub fn number(enabled: bool): int {
    if exists(maybeNumber(enabled)) { return maybeNumber(enabled) ?? 0 }
    return 0
}
```

A direct optional-producing call may have effects. When presence resolution
guards a path read, effects cannot be hidden in the read base or key
expressions. Bind an effectful result first, then read a sparse member from the
stable binding.

Presence handling does not suppress a type mismatch, invalid required data, or
decoding failure.

## Local Collection Views

| Form | Result |
|---|---|
| `keys(local)` | `List<K>` of present keys |
| `values(local)` | `List<V>` of present values |
| `count(place)` | Number of immediate present children, or scalar presence |

`keys` and `values` accept local lists and maps. They do not accept a durable
path. They materialize a new local list in key order.

`count` returns `0` for an absent empty place, `1` for a present scalar or
resource node without children, and otherwise the number of immediate present
children. On a local collection it returns the number of entries. String and
byte lengths are `std::text::length` and `std::bytes::length`.

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

pub fn fieldCount(row: string): int {
    return length(split(row, ","))
}

pub fn rejoin(row: string): string {
    return join(split(row, ","), ";")
}
```

## Collections

The finite collection types `List<T>` and `Map<K, V>` are built and read through a
closed set of procedural built-ins — there is no method syntax. `List()`/`Map()`
construct an empty collection of the expected type; `append`/`insert` add elements
and yield the updated collection (collections are values); `get` looks a key up;
`length`/`isEmpty` report size. See
[Lists and maps](types-and-values.md#lists-and-maps) for the full table and value
semantics.

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
operations `append`, `insert`, `get`, and `length` are deliberately *not*
reserved — they are common verbs — so a same-module function of one of those
names is admitted, wins at every call site in that module, and totally shadows
the built-in collection operation there.

## Positional Append

`append(place, value): int` writes after the greatest populated positive integer
position of a durable one-`int` keyed leaf and returns the written 1-based
position. An empty leaf starts at `1`; holes are not filled. (The `append` that
grows a local `List` is the collection form above, which yields the updated
list.)

```mw
module docs::builtins_append

resource Book {
    required title: string
    tags[pos: int]: string
}

store ^books[id: int]: Book

pub fn addTag(id: Id(^books), tag: string): int {
    return append(^books[id].tags, tag)
}
```

`append` is a write even though it returns a value, so it is legal only in a
transaction region.

## Entry Identities

`nextId(^root): Id(^root)` returns the current next integer identity candidate
for a single-`int` keyed root. It does not reserve or create the candidate.

`Id(^root, key...): Id(^root)` constructs an identity from explicit declared
keys without reading the store.

`key(id)` returns the raw key of an identity whose store has exactly one key
column. It is rejected for a composite identity.

## Output

`print(value)` writes one renderable value followed by a newline. Scalars,
enums, entry identities, and lists of renderable elements are supported.
Resources and local or durable trees have no direct print representation.

Interpolation and `print` use the same scalar renderings. Bytes render as
`0x`-prefixed lowercase hexadecimal; temporal values use canonical text; enums
use `Enum::member`.

`print` returns no value and is forbidden inside a transaction.

## Conversion

Conversions use call syntax:

```text
bool(value)
int(value)
decimal(value)
string(value)
bytes(value)
ErrorCode(value)
```

Each accepts a fixed set of source types and validates range or text form.
Failure raises a typed runtime error. No conversion is implicit.

`ErrorCode(value)` validates dotted lowercase code text. `string(value)` uses
canonical scalar or enum rendering. `std::bytes::toText` is the explicit UTF-8
decode operation; converting bytes to `string` does not perform that decode.

## Temporal

The temporal types `date`, `instant`, and `duration` are constructed from a
static canonical text literal (`date("2026-07-15")`,
`instant("2026-07-15T17:00:00Z")`, `duration("PT3600S")`), validated at compile
time. Two named functions provide date arithmetic:

```text
date_add_days(d: date, n: int): date
date_days_between(a: date, b: date): int
```

`date_add_days` returns the date `n` days after `d`; `date_days_between` returns
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

`delete place` is a statement, not a call. It removes the addressed value and
descendants under the rules in [Durable places](durable-places.md#deletion).
Local collection elements may also be deleted.
