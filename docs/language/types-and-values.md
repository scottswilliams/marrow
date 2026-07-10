# Types And Values

Marrow is statically typed. A checked expression has one type, and assignment,
arguments, and return values require compatible types. The language does not
perform implicit numeric or text conversions.

## Primitive Scalar Types

| Type | Values |
|---|---|
| `int` | Signed 64-bit integers |
| `bool` | `true` or `false` |
| `string` | UTF-8 text |
| `bytes` | Byte sequences |
| `decimal` | Exact base-10 values with at most 34 significant digits and 34 fractional places |
| `date` | Calendar dates in years 0001 through 9999 |
| `instant` | Nanosecond UTC instants in years 0001 through 9999 |
| `duration` | Signed fixed elapsed nanoseconds in the `i128` range |

## Nominal Values

An enum value belongs to one declared enum and names one of its members.
`Id(^root)` identifies an entry under one declared store root. These are nominal
values rather than primitive scalars: values from different enum declarations
or store roots are different types even when their stored representations have
the same shape.

`unknown` is an explicit dynamic boundary. A local `unknown` binding can carry a
value whose static shape is unresolved, but operations that require a concrete
type still reject it until a checked conversion or boundary supplies one.
Durable fields, keys, and collection elements cannot use `unknown`.

## Operators

Operators require the operand combinations below; Marrow does not widen or mix
numeric types implicitly.

| Form | Accepted operands | Result |
|---|---|---|
| `-value` | `int` or `decimal` | same type |
| `not value` | `bool` | `bool` |
| `a + b` | matching numeric types; `string`; `duration`; `instant + duration` | matching type, or `instant` |
| `a - b` | matching numeric types; `duration`; `instant - duration`; `instant - instant` | matching type, `instant`, or `duration` |
| `a * b` | matching `int` or matching `decimal` | matching type |
| `a / b` | matching `int` or matching `decimal` | `decimal` |
| `a % b` | `int` and `int` | `int` |
| `<`, `<=`, `>`, `>=` | matching `int`, `decimal`, `string`, `bytes`, `date`, `instant`, or `duration` | `bool` |
| `==`, `!=` | matching scalars, the same enum type, or identities for the same store root | `bool` |
| `and`, `or` | `bool` and `bool` | `bool` |
| `optional ?? fallback` | compatible present-arm types | presence follows the fallback |
| `value is Enum::member` | an enum value and one member or category of that enum | `bool` |

Whole resources, sequences, keyed collections, and `Error` values have no
equality operator. Arithmetic overflow, invalid division, and invalid temporal
arithmetic raise typed runtime errors. Range endpoint and step combinations are
defined under [Traversal and indexes](traversal-and-indexes.md#ranges).

## Explicit Conversion

The scalar conversion names `bool`, `int`, `decimal`, `string`, `bytes`, `date`,
`instant`, `duration`, and `ErrorCode` use call syntax. Conversions validate
their input and fail at runtime when the value cannot be represented.
`Id(^root, keys...)` uses the same call shape but constructs a nominal entry
identity rather than converting one scalar type to another.

```mw
module docs::conversion

pub fn normalized(raw: string): string
    const amount: int = int(raw)
    const rendered: string = string(amount)
    return rendered

pub fn checkedCode(raw: string): ErrorCode
    return ErrorCode(raw)
```

| Target | Accepted source values |
|---|---|
| `bool` | `bool`, or `int` equal to `0` or `1` |
| `int` | `int`, canonical integer text, or an integral `decimal` in range |
| `decimal` | `decimal`, `int`, or canonical decimal text in range |
| `string` | Any scalar or enum value, rendered canonically |
| `bytes` | `bytes`, or the UTF-8 bytes of a `string` |
| `date` | `date` or validated date text |
| `instant` | `instant` or validated instant text |
| `duration` | `duration` or validated duration text |
| `ErrorCode` | validated `string` text |

`ErrorCode` is represented as a string value. `ErrorCode(text)` requires two or
more nonempty dot-separated segments containing lowercase ASCII letters,
digits, or `_`. Explicit initialization of a non-optional annotated local and
construction or assignment of a declared resource `ErrorCode` member apply the
same validation.

The current checker does not preserve this refinement through function
parameters and returns, later reassignment of a bare local, local collection
elements, key columns, optional local initialization, an uninitialized
`ErrorCode` variable, or a module-level constant. Those positions can behave as
plain `string`; an uninitialized variable starts as the empty string. This is an
implementation limitation, not a separate persisted scalar representation.

## Optional Values

`T?` contains either a present `T` or `absent`. Optional types do not nest.
Fields, key columns, keyed leaf declarations, and sequence element declarations
cannot themselves be optional; `sequence[T]?` is valid because the optionality
applies to the sequence value.

Optional values arise from sparse reads, lookup operations, optional returns,
and optional standard-library functions. Four constructs consume them:

- `value ?? fallback` selects the present value or a fallback.
- `if const name = value` enters its block and binds `name` only when the value
  is present.
- `exists(place)` tests path presence and narrows the guarded path.
- `value?.member` propagates absence while reading a resource member.

`if const` accepts any optional expression. It is not limited to durable reads.

```mw
module docs::optionals

fn maybeLabel(enabled: bool): string?
    if enabled
        return "enabled"
    return absent

pub fn show(enabled: bool)
    if const label = maybeLabel(enabled)
        print(label)
    else
        print("disabled")
```

The optional-producing call is evaluated once. An optional expression used as
a key, address base, or durable read address must still satisfy the checker
rules for that context; a call is not rejected merely because it is a
user-defined function.

## Presence And Narrowing

A durable resource read is optional until the path is known present. A required
member is guaranteed only after its containing resource is present. For
example, `^books(id).title` has type `string?` if `^books(id)` might be absent,
even when `title` is declared `required`.

```mw
module docs::presence

resource Book
    required title: string
    subtitle: string

store ^books(id: int): Book

pub fn display(id: int)
    if exists(^books(id).subtitle)
        print(^books(id).subtitle)

    if const book = ^books(id)
        print(book.title)
        print(book.subtitle ?? "(no subtitle)")
```

Within the first branch, the exact guarded place is present. Within the second,
the materialized `Book` has a bare required `title` and an optional sparse
`subtitle`.

Presence knowledge is invalidated when code may change the relevant address or
data. This includes mutation of a key expression, a write to the guarded durable
path, or a helper call whose effects may write it. Code must test presence again
after invalidation.

## Resources

A resource value has the declared resource type. Required fields are bare in a
materialized resource; sparse fields remain optional. Unkeyed nested groups are
members of the containing value. Keyed child layers are collections addressed
separately and are not materialized as part of a local resource value.

Resource values are copied by value through local bindings, parameters, and
returns. See [Resources](resources.md).

## Sequences And Keyed Collections

`sequence[T]` is a local or declared positional collection whose keys are
positive integers. A declaration such as `var scores(name: string): int`
creates a local keyed collection. Declared resource layers and keyed local
collections may have several typed key columns and may contain resource entries
rather than scalar leaves.

A collection read at a key has type `T?` because the position may be absent.
Iteration visits only present entries. Sequences may be passed and returned by
value. A keyed local collection may be passed to a parameter with the same keyed
shape, but the language has no keyed-collection return-type syntax.

## Key Types

Store identity columns, resource keyed layers, and local keyed collections
accept `int`, `bool`, `string`, `bytes`, `date`, `instant`, and `duration` keys.
The `ErrorCode` spelling is also accepted, but current key compilation erases
its refinement: the key behaves as `string` and is not grammar-validated.
`decimal`, enums, entry identities, resources, sequences, optionals, and
`unknown` are not accepted in those key positions. Declared index components may
also project enum and entry-identity fields; `decimal` remains unavailable.

Key order is part of observable traversal behavior. Numeric and temporal keys
use their natural order, `false` precedes `true`, and strings and bytes use
lexicographic order. Composite key tuples are ordered lexicographically by
column. Enum-valued index components follow member declaration order; an
identity-valued component follows the addressed root's key-tuple order.

## Entry Identity

`Id(^root)` is nominally tied to its store root. `Id(^books)` and
`Id(^authors)` are different types even if both roots use integer keys.
Composite store keys still produce one `Id(^root)` value.

`Id(^root, keys...)` wraps explicit key values as an entry identity. It does not
read the store and does not establish that the entry exists. Stored identity
values do not create a cascading relationship: deleting the addressed entry
does not rewrite other values that contain its identity. `marrow data integrity`
can report such dangling identities.

## Error Values

`Error` is the built-in resource value constructed by `Error(...)`, returned
like another resource, or transferred with `throw`. Its fields, construction
rules, catchability, and transaction interaction are defined in
[Errors and transactions](errors-and-transactions.md).

## Mutability

Type and mutability are separate. `const` prevents reassignment of its binding;
`var` permits reassignment. A local resource held by `var` permits member
assignment. Durable places are assigned directly and are not made mutable by a
local binding.
