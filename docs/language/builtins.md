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

fn maybeNumber(enabled: bool): int?
    if enabled
        return 4
    return absent

pub fn number(enabled: bool): int
    if exists(maybeNumber(enabled))
        return maybeNumber(enabled) ?? 0
    return 0
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
| `keys(local)` | `sequence[K]` of present keys |
| `values(local)` | `sequence[V]` of present values |
| `count(place)` | Number of immediate present children, or scalar presence |

`keys` and `values` accept local sequences and local keyed collections. They do
not accept a durable path. They materialize a new local sequence in key order.

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

```mw
module docs::text_floor

pub fn isBlank(s: string): bool
    return isEmpty(trim(s))
```

## Collections

The finite collection types `List[T]` and `Map[K, V]` are built and read through a
closed set of procedural built-ins — there is no method syntax. `List()`/`Map()`
construct an empty collection of the expected type; `append`/`insert` add elements
and yield the updated collection (collections are values); `get` looks a key up;
`length`/`isEmpty` report size. See
[Lists and maps](types-and-values.md#lists-and-maps) for the full table and value
semantics.

```mw
module docs::collection_builtins

pub fn size(): int
    var xs: List[int] = List()
    xs = append(xs, 1)
    xs = append(xs, 2)
    return length(xs)
```

`isEmpty` also accepts a `string` (the text floor form above). `length` reports a
collection's element or entry count; a string's length is `std::text::length`.

## Ordered Neighbors

`next(place)` and `prev(place)` return an optional neighboring key or entry
identity:

```text
next(^books)             ; first stored book identity
prev(^books)             ; last stored book identity
next(^books(id))         ; identity after id
prev(^books(id).tags(4)) ; populated tag position before 4
```

Neighbors follow the same typed key order as traversal and skip holes. A bare
layer selects an edge entry. Stepping beyond an edge returns `absent`. These
calls are stateless and do not create a cursor.

## Sequence Append

`append(collection, value): int` writes after the greatest populated positive
integer position and returns the written 1-based position. An empty collection
starts at `1`; holes are not filled. It accepts a local sequence or a durable
one-`int` keyed leaf.

```mw
module docs::builtins_append

resource Book
    required title: string
    tags(pos: int): string

store ^books(id: int): Book

pub fn addTag(id: Id(^books), tag: string): int
    return append(^books(id).tags, tag)
```

`append` is a write even though it returns a value. Transaction and traversal
write restrictions apply.

## Entry Identities

`nextId(^root): Id(^root)` returns the current next integer identity candidate
for a single-`int` keyed root. It does not reserve or create the candidate.

`Id(^root, key...): Id(^root)` constructs an identity from explicit declared
keys without reading the store.

`key(id)` returns the raw key of an identity whose store has exactly one key
column. It is rejected for a composite identity.

## Output

`print(value)` writes one renderable value followed by a newline. Scalars,
enums, entry identities, and sequences of renderable elements are supported.
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
date(value)
instant(value)
duration(value)
ErrorCode(value)
```

Each accepts a fixed set of source types and validates range or text form.
Failure raises a typed runtime error. No conversion is implicit.

`ErrorCode(value)` validates dotted lowercase code text. `string(value)` uses
canonical scalar or enum rendering. `std::bytes::toText` is the explicit UTF-8
decode operation; converting bytes to `string` does not perform that decode.

## Errors

A recoverable failure a program handles is an ordinary `Result[T, E]` value (see
[Types and values](types-and-values.md)), returned with `err(...)` and propagated
with prefix `try`. There is no throwable error value and no `throw`/`catch`
channel; the distinct failure kinds are described in
[Errors and transactions](errors-and-transactions.md).

## Deletion

`delete place` is a statement, not a call. It removes the addressed value and
descendants under the rules in [Durable places](durable-places.md#deletion).
Local collection elements may also be deleted.
