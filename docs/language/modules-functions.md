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

Spell `maybe` before a function return type when the function may return absence
instead of a value:

```mw
fn findSubtitle(id: int): maybe string
    return ^books(id).subtitle
```

`maybe` is valid only in this return-type position. It is not a parameter,
field, saved-data, local, keyed-tree, or nested-data type wrapper.

Every value-returning function, including a maybe-returning function, must return
on every reachable path. Plain fall-through is a missing-return error. Inside a
maybe-returning function, `return absent` exits with absence:

```mw
fn pick(flag: bool): maybe int
    if flag
        return 1
    return absent
```

`return absent` is valid only as the entire return expression of a
maybe-returning function. Plain `return` in a maybe-returning function is still a
return-value error.

A maybe-returning function may propagate absence by returning a maybe-present
saved read or another maybe-returning call:

```mw
fn inner(id: int): maybe string
    return ^books(id).subtitle

fn outer(id: int): maybe string
    return inner(id)
```

The caller resolves the maybe-present result at its own call site with `??`, `if
const`, or `exists(...)`. An unresolved maybe-returning call is a compile error.

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
same identity-key guards used by saved data. Resource-shaped parameters, local
tree parameters, group entries, unknown or invalid parameter surfaces,
unsupported sequence element types, and composite identity keys are outside the
CLI entry surface; composite identity parameters should be wrapped by an entry
that accepts the scalar key parts and constructs or looks up the identity in
Marrow code.
Decoding is per invocation: scalar and enum text is parsed once per supplied
argument, repeated sequence values are appended in argv order, and identity
keys run the same key guard used by saved data before the function starts.

Plain text `marrow run` leaves program `print` output on stdout. With
`--format json`, the CLI captures that output into a result envelope and renders
the return value only when it has a JSON surface. Identity returns use the same
JSON identity form as saved-data JSON tooling. Resource-shaped returns are
excluded from the run JSON surface.

## Parameters

Parameters are read-only by-value inputs:

```mw
fn format(book: Book): string
    return $"{book.title} by {book.author}"
```

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

Module-level functions, constants, enums, resources, and imported short module
names share one namespace inside a module. A module cannot declare a function
and a resource with the same name.

Names resolve in this order:

1. local declarations and parameters,
2. imported module names,
3. declarations in the current module,
4. builtins.

Module-level declarations — functions, constants, enums, and resources — cannot
redefine builtin names such as `exists`, `keys`, `Error`, `print`, or `int`. An
imported short module name binds the import even when it matches a builtin name,
shadowing the builtin within the file. Local variables may also shadow builtin
names.

## Host Boundaries

Host integrations can expose outside services through explicit modules.
General `.mw` code uses `.mw` modules and `std::` libraries.
