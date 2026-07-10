# Modules And Functions

Modules organize code. Functions operate on primitive values, resources, local
trees, and saved trees.

## Modules

Reusable files declare a module:

```mw
module shelf::books
```

`::` separates code namespaces. Dots are for data fields.

The v0.1 module layout is one file per module under project source roots.
For example, module `shelf::books` lives at `shelf/books.mw` below one source
root. The `module` declaration must match the source-root-relative path.

External package management is outside this reference.

A file without a `module` declaration is a single-file script. It can run as
an entrypoint, but other modules do not import it by path. A script's own
declarations are type-checked within the file; other modules cannot import a
script, and its functions are not callable from another module. A project may
hold at most one such script; every other file declares its module name
explicitly.

## Imports

```mw
use std::clock
use shelf::books

const now: instant = clock::now()
const id = books::add(
    title: "Small Gods",
    author: "Terry Pratchett",
    shelf: "fiction",
    changedAt: now,
)
```

Fully qualified calls are always valid:

```mw
const now: instant = std::clock::now()
```

`use` imports a module name. It does not copy that module's declarations into
the current namespace. After `use shelf::books`, code may call `books::add`.
If two imports would create the same short module name, use a fully qualified
name instead.
An imported short module name also cannot collide with a module-level
function, constant, enum, or resource in the current module.

The v0.1 import surface has no wildcard imports, renamed imports, or path
imports.

## Visibility

Omitted function visibility is module-private. Function `pub` marks callable
API: another module, the CLI, or a host embedding can call the function.

```mw
pub fn add(title: string): int
fn normalize(title: string): string
fn rebuildIndex()
```

Marrow does not add separate `private` or `internal` keywords in v0.1. Keep the
boundary simple: public or module-private.

Project and CLI entrypoints in module files use `pub fn`.

Top-level constants are private to their module.

They are compile-time constant expressions over literals and other top-level
constants. They do not read saved data or call host modules. Local immutable
values use `const`.

Resource declarations do not take visibility markers. Resources are not
visibility-gated in v0.1: a resource belongs to its module and can be named with
that module path where the project schema is loaded. Function visibility
controls the callable API.

## Functions

Function declarations use one `fn` form:

```mw
fn add(title: string, author: string): int
    return 1
```

An omitted return type means the function produces no value:

```mw
fn remove(id: Id(^books))
    delete ^books(id)
```

Write a return type as `T?` when the function may yield absence instead of a
value. `T?` is the optional type — a `T` or absence (see
[Types](types.md#optional-types-t)):

```mw
fn findSubtitle(id: int): string?
    return ^books(id).subtitle
```

The `?` suffix is a first-class code type: it is equally valid on a parameter and
a local annotation. It never marks a field, key, keyed leaf, or sequence element,
where a sparse slot already provides absence.

Every value-returning function must return on every reachable path; plain
fall-through is a missing-return error. `absent`, the empty optional, is an
ordinary value assignable to any `T?` place, so a `T?` function may yield it
directly or return a maybe-present read that carries absence outward:

```mw
fn inner(id: int): string?
    return ^books(id).subtitle

fn outer(id: int): string?
    return inner(id)
```

The caller resolves the `T?` result at its own call site with `??`, `if const`,
or `exists(...)`. An unresolved `T?` call is a compile error.

Functions are not overloaded by parameter type. A module has one declaration
for a function name. v0.1 has no user-defined generic functions.

Functions may read or write local data, saved data, output, and other effectful
operations. Marrow does not split functions into a separate `proc` construct.
The return type says whether the call produces a value; the body and checker
describe the effects:

```mw
fn addBook(title: string, author: string, shelf: string): Id(^books)
    var book: Book
    book.title = title
    book.author = author
    book.shelf = shelf

    const id: Id(^books) = nextId(^books)
    ^books(id) = book
    return id

fn remove(id: Id(^books))
    delete ^books(id)
```

Calls to no-return functions are statements. Calls to value-returning
functions can appear in expressions, even when the function also has visible
effects.
A value-returning function can also be called as a statement when the caller
intentionally ignores the result.

## Entrypoints

Tools call public functions by entry name. A qualified entry names one module
exactly:

```text
marrow run --entry shelf::books::main .
```

A bare entry name is accepted only when it names one public function in the
checked program. If two modules export the same function name, qualify the entry.
A module entrypoint is an ordinary `pub fn` declaration. Argument decoding and
result rendering happen at the tool or host boundary, then Marrow code runs with
typed parameters and typed returns.

For the CLI boundary, `marrow run --arg name=value` decodes arguments from the
checked function signature. Scalar text uses the same literal/runtime scalar
grammar as Marrow values, enum text uses accepted member spellings, and `string`
receives the raw text after the first `=`. A sequence parameter whose element is
scalar or enum collects repeated `--arg` values in argv order; `[]` is the empty
sequence spelling. A single-component `Id(^store)` parameter decodes through the
same entry-identity key guards used by saved data. Resource-shaped parameters, local
tree parameters, group entries, unknown or invalid parameter surfaces,
unsupported sequence element types, and composite entry identities are outside
the CLI entry surface; expose a function that accepts the scalar key parts and
constructs or looks up the composite entry identity in Marrow code instead.
Decoding is per invocation: scalar and enum text is parsed once per supplied
argument, repeated sequence values are appended in argv order, and entry identity
keys run the same key guard used by saved data before the function starts.

Plain text `marrow run` leaves program `print` output on stdout. With
`--format json`, the CLI captures that output into a result envelope and renders
the return value only when it has a JSON surface. Entry identity returns use the
same JSON form as saved-data JSON tooling. Resource-shaped returns are excluded
from the run JSON surface.

## Parameters

Parameters are read-only by-value inputs:

```mw
fn format(book: Book): string
    return $"{book.title} by {book.author}"
```

A local collection is a parameter like any other value. A `sequence[T]` and a
keyed tree both pass by value; a keyed-collection parameter is spelled like its
local declaration head, with key columns before the leaf value type:

```mw
fn total(scores(player: string): int): int
    var sum = 0
    for player in scores
        sum = sum + (scores(player) ?? 0)
    return sum
```

The argument is a caller-local collection of the same shape. A saved collection
(a store root, a saved sub-layer, or an index branch) is iterated in place, not a
local value, so it cannot fill a by-value collection slot — a parameter or a
declared collection return type alike; iterate it or build a local collection
instead. Because a parameter is read-only, a function
reads its keyed parameter but cannot write through it; return a new collection to
hand back a changed one.

Return a replacement value when a helper needs to transform a caller-local
value:

```mw
fn normalize(book: Book): Book
    var next: Book = book
    next.title = std::text::trim(next.title)
    return next

var draft: Book
if const saved = ^books(id)
    draft = saved
    draft = normalize(draft)
```

Use explicit saved assignments at the call site when saved data must change.

## Passing Resources

Resources pass like typed values by default:

```mw
fn save(id: Id(^books), book: Book)
    ^books(id) = book
```

Transforming a caller's local resource is a return-value pattern:

```mw
fn trimTitle(book: Book): Book
    var next: Book = book
    next.title = std::text::trim(next.title)
    return next
```

## Named Arguments

Calls may use positional or named arguments. Named arguments improve clarity
for options and resource constructors:

```mw
saveBook(book: draft, notify: true)

const err = Error(
    code: "book.absent",
    message: $"Book {id} does not exist.",
)
```

Positional and named arguments are not mixed after the first named argument.

## Locals And Scope

`const` and `var` are lexically scoped:

```mw
const title = "Small Gods"
var loanCount = 0

if loanCount < 5
    var status = "ok"
```

`status` exists only in the block. Redeclaring the same name in the same
block is an error. Shadowing from inner blocks is allowed.

## Name Resolution

Module-level functions, constants, enums, resources, surface declarations, and
imported short module names share one source namespace inside a module. A module
cannot declare a function and a resource with the same name.

Names resolve in this order:

1. local declarations and parameters,
2. imported module names,
3. declarations in the current module,
4. builtins.

Module-level declarations — functions, constants, enums, resources, and surfaces
— cannot redefine builtin names such as `exists`, `keys`, `Error`, `print`, or
`int`. An imported short module name binds the import even when it matches a
builtin name, shadowing the builtin within the file. Local variables may also
shadow builtin names. Surface declarations participate here as source names only;
they are not durable saved-data entities, route declarations, or callable values.
Custom application behavior remains ordinary `pub fn` code; current surface
grouping can only refer to already checked functions.

## Host Boundaries

Host integrations can expose outside services through explicit modules.
General `.mw` code uses `.mw` modules and `std::` libraries.
