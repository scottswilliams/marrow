# Builtins

Builtins are always available. Documentation uses their full names.

## Presence And Reads

`exists(path): bool` returns true when the addressed record node, value, or
child tree exists:

```mw
if exists(^books(id))
    if const title = ^books(id).title
        print(title)
```

The absence-default operator `??` reads a value when populated and otherwise
yields the default:

```mw
const subtitle = ^books(id).subtitle ?? ""
```

The optional read `?.` accesses a field that may be absent without failing the
whole read; an absent step short-circuits the rest of the chain, so a `?.` chain
paired with `??` reads a deep value with one fallback:

```mw
const shelf = ^books(id)?.binding?.shelf ?? "unshelved"
```

These are for sparse paths. They do not suppress schema or decoding errors; a
missing required field in saved data is still invalid data. The operators are
covered in detail under Operators in the syntax reference.

The same forms resolve every maybe-present value, not only saved paths: a local
positional read `xs(pos)`, a local keyed read `counts(k)`, a sparse field of a
materialized value such as `book.subtitle`, `person.address.zip`, a loop-bound
group entry's field, or a caught `err.help`, and a local `var`/`const`/parameter
of optional type (`v: string?`). A bare such read is a compile error.

The guarded expression and its key and base sub-expressions must be effect-free
reads. A write, an allocation, a host call, or any user-function call inside the
guard is rejected, whether it is the read itself (`exists(append(xs, v))`,
`exists(nextId(^s))`) or smuggled in as a key (`counts(nextId(^s)) ?? 0`,
`counts(writeBook())`), so the guard never runs a side effect.

## Collection Traversal

A durable collection — a store root, keyed child layer, or index branch — is
iterated with a `for` loop; the key-first head, single versus two loop variables,
the `reversed` direction keyword, and the lazy-streaming guarantee are described
under Loops in [Control Flow And Errors](control-flow-and-effects.md). Saved data
is never materialized as a value. The builtins below cover local materialization,
counting, and ordered neighbors.

| Builtin | Meaning |
|---|---|
| `keys(collection)` | A local sequence of a local collection's addresses |
| `values(collection)` | A local sequence of a local collection's stored values |
| `count(path)` | Populated immediate children, or path presence |
| `next(element)` | The nearest stored neighbor address in key order |
| `prev(element)` | The nearest stored neighbor address, the other way |

`keys(...)` and `values(...)` materialize a local sequence over a local
collection — a keyed `var`, a keyed parameter, or a sequence value. `keys(...)`
yields the addresses and `values(...)` yields the stored values, each a `sequence`
value that can be bound, passed, or returned. They are rejected over any saved
path: durable saved data is never materialized as a value, so iterate it with
`for ... in`. They are also rejected directly as a loop-head iterable
(`for k in keys(xs)`), where the key-first head already binds addresses; bind the
sequence first when a materialized copy is wanted.

Sequences and keyed maps are conveniences over saved tree layers, not separate
database features. Key-only collections such as non-unique index branches do
not have separate values; their generated marker values are an inspection
detail. Deep saved-data walks belong to inspection, export, repair, and data
evolution tools.

### Stored Entries In Key Order

Every form of iteration — a `for` loop, forward or `reversed`, and
`next(...)`/`prev(...)` — visits only **stored** entries, in key order, and
**skips holes**. There are no placeholder positions to step onto: deleting an
entry removes it from every traversal, and a gap left by a delete or by sparse
keys is passed over rather than visited. This is the storage guarantee the
ordered-navigation helpers below rest on.

Do not mutate the same tree layer a loop is traversing. The checker rejects
obvious cases. When a dynamic path writes the layer currently being traversed,
the runtime reports a typed traversal error. Collect keys into a local
sequence first when a rewrite needs to change the traversed layer. Generated
index maintenance counts as a write to the affected index layer.

`count(path)` returns an `int`:

- `0` when the path is not populated and has no children,
- `1` when the path is a scalar value or record node,
- the number of immediate children when the path is a tree.

If a path has both a value and children, `count` returns the number of
immediate children. Use `exists(path)` for a direct presence check.
`count(collection)` over a local sequence or keyed map likewise returns its
`int` element count, so it is usable in a typed binding and in arithmetic
exactly as over a saved path.

`count(...)` is a one-layer tree scan, not a maintained counter. For hot
paths, store an explicit counter or use a declared index.

String and byte lengths use `std::text::length(text)` and
`std::bytes::length(value)`.

### Stored Neighbors

`next(element)` returns the nearest stored entry after `element` in key order;
`prev(element)` returns the nearest stored entry before it. Both are stateless —
there is no cursor — and both skip gaps, returning the nearest entry that is
actually stored rather than the next key value:

```mw
const after = next(^books(id)) ?? id
const afterTitle = ^books(after).title ?? ""
```

For a store root, the result is the neighbor's **entry identity**, so fields are
read through it (`^books(next(^books(id))).field`). For a keyed child layer, the
result is that layer's key. Neighbors are found for any key type — string, date,
integer, and so on — through the same order-preserving key encoding tree
iteration uses.

`next` and `prev` are scoped to one key level. Applied to a bare layer they return
its edge entry: `next(^books)` is the first stored record, `prev(^books)` the
last; `next(^books(id).tags)` is the first stored position in that layer.

Stepping off the edge — `next` of the last entry, or `prev` of the first — has no
neighbor, so a store-root result types as `Id(^store)?` and must be resolved at
the read, like any `T?` value. It composes with `??`:

```mw
const following = next(^books(id)) ?? id
```

## Sequence Writes

`append(path, value)` appends to a sequence-like integer-keyed tree and returns
the 1-based key it wrote:

```mw
const pos: int = append(^books(id).tags, "fiction")
```

This writes:

```text
^books(id).tags(pos) = "fiction"
```

`append(...)` is an effectful value function. The checker tracks the write
just as it tracks direct assignment.

Append chooses one greater than the highest populated positive integer key. An
empty sequence writes key `1`. Append does not fill holes, and deleting an
entry does not renumber subsequent entries. Treat sequence keys as storage
positions, not as a promise that the sequence is dense.

## Print

`print(...)` is a call-shaped statement that writes one rendered value plus a
newline to the default output stream:

```mw
print($"saved {id}")
```

It renders every scalar, every enum, an entry identity, and any sequence whose
element type also renders. Scalars render in their canonical form: an `instant`,
`date`, or `duration` as its canonical text, and `bytes` as `0x`-prefixed
lowercase hex (matching `data dump`). An enum renders as its `Enum::member`
source spelling. A single-key entry identity renders as its key, and a
composite entry identity renders as `identity(k1, k2)`. A sequence renders as
bracketed elements in sequence order:

```mw
print(std::text::split("a,b", ","))
```

This writes:

```text
[a, b]
```

`print` and string interpolation render the same set and reject the same set: a
local tree, resource, saved collection path, or sequence whose element type does
not render has no direct text form and is a check error at the render argument
when its type is statically known. `print` produces no value. Complex IO belongs
in `std::io`.

## Delete

`delete path` removes the value and child tree at that path:

```mw
delete ^books(id).subtitle
```

`merge` is a reserved word, not a v0.1 statement; see Delete in
[Resources And Saved Data](resources-and-storage.md) for how to preserve existing
data without a whole-record `=`.

## Conversions

Conversion builtins validate dynamic values:

```mw
const id: int = int(raw)
const amount: decimal = decimal(raw)
const title: string = string(raw)
const ok: bool = bool(raw)
const payload: bytes = bytes(title)
const code: ErrorCode = ErrorCode(raw)
const day: date = date(raw)
const moment: instant = instant(raw)
const span: duration = duration(raw)
```

`raw` means a value whose static type is not known. Avoid keeping values raw
after the boundary where they enter the program. Each conversion accepts a
fixed set of static source types; [Types — Conversions](types.md#conversions)
lists the matrix.

Date, instant, and duration conversions validate canonical Marrow values. Use
`std::clock` helpers for parsing or formatting text at user and host
boundaries.

`string(...)` renders any scalar plus an enum, using the same scalar and enum
text that `print` uses: a temporal as its canonical text, `bytes` as
`0x`-prefixed lowercase hex, and an enum as its `Enum::member` spelling. It does
not accept an entry identity or a sequence, which `print` and interpolation render
directly when the value is otherwise renderable. UTF-8 decoding of bytes is the
separate `std::bytes::toText` path; `string(bytes)` never depends on valid UTF-8.

## IDs

`nextId(root)` returns the next entry identity value for a keyed store root with
the default integer identity policy:

```mw
const id = nextId(^books)
```

For a typed store root, `nextId` returns that store's entry identity type:

```mw
const id: Id(^books) = nextId(^books)
```

`nextId(...)` returns the next-available entry identity — the current maximum
plus one. It does not advance the allocation until a record is actually written, so two
`nextId(^root)` calls with no write to that store between them return the *same*
value. Allocate, write, then allocate again to obtain distinct ids:

```mw
const a = nextId(^books)
^books(a) = first
const b = nextId(^books)
^books(b) = second
```

Binding two ids and writing both without an intervening write inserts the same
record twice; the checker warns (`check.next_id_collision`).

Marrow provides a default per-root allocation policy for a store with one `int`
identity key. Composite entry identities and non-integer entry identities are
application-provided; `nextId` is not available for those roots in ordinary
`.mw`.

Entry identities are opaque and may have gaps, including gaps left by failed transactions.
Do not use them as business counters.

After restore, `nextId` must choose an unused entry identity. It may skip ahead.

If a store uses application-provided identity keys, allocate or validate those
keys at the application boundary, then wrap them with the explicit entry identity
constructor before writing the resource:

```mw
const id: Id(^books) = Id(^books, "book-17")
```

`Id(^store, key...)` performs no allocation and no lookup. It only constructs an
entry identity value after the key argument count and scalar types match the
store's declared identity keys. A constructed entry identity is not a presence
proof; the first saved read still resolves absence in the ordinary way.

`key(id)` projects an entry identity back to its scalar key, the inverse of the
single-argument `Id(^store, key)`:

```mw
for id in ^tags
    print(key(id))
```

It is defined only for a store with a single identity key, and returns that key's
type. A composite entry identity is reconstructed as a whole value, never
exposed as a tuple of raw key components, so `key` over a root with a composite
entry identity is rejected at check.

## Errors

`Error(...)` constructs a builtin error resource value:

```mw
const err = Error(
    code: "book.absent",
    message: "Book does not exist.",
)
```

`throw err` raises it:

```mw
throw err
```

`throw Error(...)` is the common inline form.
