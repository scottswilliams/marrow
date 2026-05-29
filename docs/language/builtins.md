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

## Tree Traversal

Direct iteration over a tree yields keys from the next layer:

```mw
for id in ^books
    write($"{id}")
```

For managed roots, iteration follows the declared layer. `^books` yields book
identities; `^books.byShelf("fiction")` yields the identities stored in that
index branch.

On declared index branches, use direct iteration or `keys(...)` to read
identities. `values(...)` and `entries(...)` are for primary resource roots
and ordinary keyed layers; generated index marker values are a raw inspection
detail.

Explicit traversal helpers:

| Builtin | Meaning |
|---|---|
| `keys(tree)` | Keys at the next layer |
| `values(tree)` | Values/resources at the next layer |
| `entries(tree)` | Key and value/resource pairs |
| `count(path)` | Populated immediate children, or scalar presence |

`keys(...)` is the lightest traversal shape when code only needs identities or
child keys. `values(...)` and `entries(...)` materialize the values or
resources they yield. Deep raw tree walks belong to inspection, backup, repair,
and migration tools.

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

## Sequence Updates

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

For local trees and single-writer saved profiles, append is ordinary tree
write work. In a shared-writer capability profile, append requires a lock or
backend capability that can reserve a unique child key. If Marrow cannot
choose a key safely, it reports a typed capability or runtime error instead of
guessing.

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

## Delete And Merge

`delete path` removes the value and child tree at that path:

```mw
delete ^books(id).subtitle
```

`merge target = source` copies a tree:

```mw
var draftBook: Book
merge draftBook = ^books(id)
```

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
const at: instant = instant(raw)
const span: duration = duration(raw)
```

`raw` means a value whose static type is not known. Avoid keeping values raw
after the boundary where they enter the program.

Date, instant, and duration conversions validate canonical Marrow values. Use
`std::clock` helpers for parsing or formatting text at user and host
boundaries.

## IDs

`nextId(root)` returns the next identity value for a keyed saved resource root
with the default integer identity policy:

```mw
const id = nextId(^books)
```

For a typed resource root, `nextId` returns that resource's ID type:

```mw
const id: Book::Id = nextId(^books)
```

`nextId(...)` is an effectful value function. The checker tracks the
allocation just as it tracks `append(...)` and direct saved writes.

Marrow provides a default per-root allocation policy for a resource with one
`int` identity key. Composite identities and non-integer identities are
application-provided; `nextId` is not available for those roots in ordinary
`.mw`.

IDs are opaque and may have gaps, including gaps left by failed transactions.
Do not use them as business counters. If the selected capability profile cannot
guarantee safe integer ID allocation, Marrow reports a typed capability error
before running, or a typed runtime error if the missing promise is discovered
late.

After restore, `nextId` must choose an unused identity. It may skip ahead.

If a resource root uses application-provided identity keys, allocate those keys
in application or host code, then construct the generated identity value before
writing the resource:

```mw
const id = Book::Id(17)
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
