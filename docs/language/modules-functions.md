# Modules And Functions

Modules organize code. Functions operate on primitive values, resources, local
trees, and saved trees.

## Modules

Reusable files declare a module:

```mw
module shelf::books
```

`::` separates code namespaces. Dots are for data fields.

The first module layout is one file per module under project source roots.
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
function, constant, or resource in the current module.

The first import surface has no wildcard imports, renamed imports, or path
imports.

## Visibility

Omitted visibility is module-private. Add `pub` when another module, the CLI,
or a host embedding can call the function.

```mw
pub fn add(title: string): int
fn normalize(title: string): string
fn rebuildIndex()
```

Marrow does not add separate `private` or `internal` keywords in the first
module system. Keep the boundary simple: public or module-private.

Project and CLI entrypoints in module files use `pub fn`.

Top-level constants are private to their module.

They are compile-time constant expressions over literals and other top-level
constants. They do not read saved data or call host modules. Local immutable
values use `const`.

Resource declarations do not take visibility markers. A resource belongs to
its module and can be named with that module path where the project schema is
loaded. Function visibility controls the callable API.

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

Every value-returning function must return a value on every reachable path.
Functions are not overloaded by parameter type. A module has one declaration
for a function name. The first language surface has no user-defined generic
functions.

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

Tools call public functions by qualified name:

```text
marrow run --entry shelf::books::main
```

A module entrypoint is an ordinary `pub fn` declaration. Argument decoding and
result rendering happen at the tool or host boundary, then Marrow code runs
with typed parameters and typed returns.

## Parameters

Parameters are input/read-only by default:

```mw
fn format(book: Book): string
    return $"{book.title} by {book.author}"
```

Use `out` for values written by the callee:

```mw
fn parseInt(text: string, out value: int): bool
```

Callers must mark the argument:

```mw
var n: int
if parseInt(input, out n)
    write($"parsed {n}")
```

The marked `out` argument must be a writable place: a local variable, a field of
a local value, or a saved path. It cannot be an arbitrary expression.
The checker requires every `out` parameter to be assigned before every normal
return.

Use `inout` when the callee reads and mutates the caller's value:

```mw
fn normalize(inout book: Book)
var draft: Book = ^books(id)
normalize(inout draft)
```

The `inout` marker at the call site makes caller-visible writes explicit.
An `inout` argument is a writable local place, not a hidden reference value.
Saved paths are not valid `inout` arguments. Use explicit saved assignments at
the call site when saved data must be updated.

## Passing Resources

Resources pass like typed values by default:

```mw
fn save(id: Id(^books), book: Book)
    ^books(id) = book
```

Mutation of the caller's local resource requires `inout`:

```mw
fn trimTitle(inout book: Book)
    book.title = std::text::trim(book.title)
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

Module-level functions, constants, resources, and imported short module names
share one namespace inside a module. A module cannot declare a function and a
resource with the same name.

Names resolve in this order:

1. local declarations and parameters,
2. imported module names,
3. declarations in the current module,
4. builtins.

Module-level functions, constants, resources, and imported short module names
cannot redefine builtin names such as `exists`, `keys`, `Error`, `write`, or
`int`. Local variables may shadow builtin names.

## Host Boundaries

Host integrations can expose outside services through explicit modules.
General `.mw` code uses `.mw` modules and `std::` libraries.
