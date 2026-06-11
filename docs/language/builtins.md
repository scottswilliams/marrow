# Builtins

Builtins are always available. Documentation uses their full names.

## Presence And Reads

`exists(path): bool` returns true when a value or child tree exists:

```mw
if exists(^books(id))
    write(^books(id).title)
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

## Collection Traversal

A durable collection — a store root, keyed child layer, or index branch — is an
iterable. The `for`-loop forms that walk one, including single versus two loop
variables and the lazy-streaming guarantee, are described under Loops in
[Control Flow And Errors](control-flow-and-effects.md). The traversal builtins
below are the expression forms of the same walk.

| Builtin | Meaning |
|---|---|
| `keys(collection)` | Element addresses |
| `values(collection)` | Stored values |
| `entries(collection)` | Address and stored-value pairs |
| `count(path)` | Populated immediate children, or scalar presence |
| `reversed(iterable)` | The same iterable shape in reverse key order |
| `next(element)` | The nearest stored neighbor identity in key order |
| `prev(element)` | The nearest stored neighbor identity, the other way |

`keys(...)` is the lightest traversal shape when code only needs identities,
positions, map keys, or other addresses. Over a durable collection these
builtins are loop-iterable forms only: materializing durable saved data as a
value is rejected, so iterate the result directly. Over a local collection,
`keys(...)` yields an address sequence that can be passed around as a value.
`values(...)` and `entries(...)` explicitly read stored values. Sequences and keyed maps are conveniences over saved tree layers,
not separate database features. Key-only collections such as non-unique index
branches do not have separate values; their generated marker values are an
inspection detail. Deep saved-data walks belong to inspection, export, repair,
and data evolution tools.

### Stored Entries In Key Order

Every form of iteration — `for`, `keys(...)`, `values(...)`, `entries(...)`,
`reversed(...)`, and `next(...)`/`prev(...)` — visits only **stored** entries, in
key order, and **skips holes**. There are no placeholder positions to step onto:
deleting an entry removes it from every traversal, and a gap left by a delete or
by sparse keys is passed over rather than visited. This is the storage guarantee
the ordered-navigation helpers below rest on.

Do not mutate the same tree layer a loop is traversing. The checker rejects
obvious cases. When a dynamic path writes the layer currently being traversed,
the runtime reports a typed traversal error. Collect keys into a local
sequence first when a rewrite needs to change the traversed layer. Generated
index maintenance counts as a write to the affected index layer.

`count(path)` returns:

- `0` when the path is not populated and has no children,
- `1` when the path is a scalar value,
- the number of immediate children when the path is a tree.

If a path has both a value and children, `count` returns the number of
immediate children. Use `exists(path)` for scalar presence.

`count(...)` is a one-layer tree scan, not a maintained counter. For hot
paths, store an explicit counter or use a declared index.

String and byte lengths use `std::text::length(text)` and
`std::bytes::length(value)`.

### Reverse Iteration

`reversed(iterable)` yields the same elements as the iterable in reverse key
order. It works over a layer or index branch directly, over `keys(...)` of either,
over `values(...)` and `entries(...)` where those apply, and over an in-memory
`sequence`:

```mw
for id in reversed(^books)
    write(^books(id).title)

for pos in reversed(^books(id).tags)
    write(^books(id).tags(pos))

for word in reversed(std::text::split(line, ","))
    write(word)
```

Over a saved layer the reversal streams stored keys from high to low — it is a
true reverse, not a copy of the forward result reversed after the fact. An early
`break` stops the scan. A composite identity reverses at every key level, so
`reversed(^enrollments)` is the exact reverse of `^enrollments`, not its
outermost key flipped over a forward tail. Over a `sequence` value, the elements
are reversed directly.

### Stored Neighbors

`next(element)` returns the nearest stored entry after `element` in key order;
`prev(element)` returns the nearest stored entry before it. Both are stateless —
there is no cursor — and both skip gaps, returning the nearest entry that is
actually stored rather than the next key value:

```mw
const afterTitle = ^books(next(^books(id))).title
```

The result is the neighbor's **identity**, addressed like any key, so fields are
read through it (`^books(next(^books(id))).field`). Neighbors are found for any
key type — string, date, integer, and so on — through the same order-preserving
key encoding tree iteration uses.

`next` and `prev` are scoped to one key level. Applied to a bare layer they return
its edge entry: `next(^books)` is the first stored record, `prev(^books)` the
last; `next(^books(id).tags)` is the first stored position in that layer.

Stepping off the edge — `next` of the last entry, or `prev` of the first — has no
neighbor, so the result is maybe-present and must be resolved at the read, the same
as any maybe-present value. It composes with `??`:

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

## Write And Print

`write(...)` is a call-shaped statement that writes text to the default output
stream:

```mw
write($"book {id}: {title}")
```

`print(...)` is a call-shaped statement that writes text plus a newline:

```mw
print($"saved {id}")
```

Neither statement produces a value. Complex IO belongs in `std::io`.

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

## IDs

`nextId(root)` returns the next identity value for a keyed store root with the
default integer identity policy:

```mw
const id = nextId(^books)
```

For a typed store root, `nextId` returns that store's identity type:

```mw
const id: Id(^books) = nextId(^books)
```

`nextId(...)` is an effectful value function. The checker tracks the
allocation just as it tracks `append(...)` and direct saved writes.

Marrow provides a default per-root allocation policy for a store with one `int`
identity key. Composite identities and non-integer identities are
application-provided; `nextId` is not available for those roots in ordinary
`.mw`.

IDs are opaque and may have gaps, including gaps left by failed transactions.
Do not use them as business counters.

After restore, `nextId` must choose an unused identity. It may skip ahead.

If a store uses application-provided identity keys, allocate those keys in
application or host code, then pass the checked identity value before writing the
resource:

```mw
const id: Id(^books) = loadBookId("book-17")
```

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
