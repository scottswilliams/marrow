# Modules And Functions

Modules provide source namespaces. Functions are typed, by-value calls that may
operate on local values and durable places.

## Module Files

A reusable file starts with:

```text
module shelf::books
```

The module path is the source-root-relative file path with `/` replaced by
`::` and `.mw` removed. The declaration in `shelf/books.mw` must therefore be
`module shelf::books`. One file defines one module.

A file without a `module` declaration is a single-file script. It is checked in
its own namespace and is not imported by module path.

## Imports

`use` imports a module path and binds its final segment as the local module
name:

```text
use std::clock
use shelf::books

fn example()
    const today = clock::today()
    const id = books::add("Small Gods")
```

The import does not place individual declarations into the local namespace.
Fully qualified calls such as `std::clock::today()` remain valid. There are no
wildcard or explicitly renamed imports. Two imports with the same final segment
are ambiguous.

## Visibility

A function without `pub` is callable only within its module. `pub fn` is
callable from other modules and host entry boundaries. Top-level constants are
module-private. Resource declarations participate in the project type
namespace and do not take a visibility marker. Enum declarations may be public
when their members appear across a module boundary.

Function visibility controls calls; it does not grant or restrict access to a
durable root. Current store declarations are project-wide.

## Declarations And Returns

```mw
module docs::functions

resource Book
    required title: string

fn normalize(book: Book): Book
    var result: Book = book
    result.title = std::text::trim(result.title)
    return result

fn maybeTitle(show: bool): string?
    if show
        return "Marrow"
    return absent

pub fn title(show: bool): string
    return maybeTitle(show) ?? "(hidden)"
```

Parameters are named and typed. An omitted return type means the function
returns no value. Every reachable path of a value-returning function must
return. An optional return `T?` may return either `T` or `absent`.

Marrow does not overload functions and has no user-defined generic functions.
A module has at most one function with a given name.

## Parameters Are By Value

Scalars, resources, and sequences are passed by value. A local keyed collection
may be passed to a parameter with the same keyed shape, but it has no return-type
syntax. Parameters cannot be assigned. A helper that changes a local resource or
sequence returns the replacement value; keyed collection parameters are
read-only.

```mw
module docs::parameters

fn total(scores(player: string): int): int
    var sum = 0
    for player, score in scores
        sum += score
    return sum

pub fn example(): int
    var scores(player: string): int
    scores("Ada") = 8
    scores("Lin") = 13
    return total(scores)
```

A durable root, durable child layer, or index branch is not a by-value
collection. Traverse it at its address, or copy selected entries into a local
collection.

## Arguments

Project-function arguments may be positional:

```text
add("Small Gods", "Terry Pratchett")
```

or named:

```text
add(title: "Small Gods", author: "Terry Pratchett")
```

After the first named argument, remaining arguments must be named. Named
arguments are matched to project-function parameter names, and a trailing comma
is accepted in a multiline call. Resource constructors and `Error` use named
fields. Conversions, `Id`, language built-ins, and `std::` operations use
positional arguments.

Calls are evaluated from left to right. A value-returning function may also be
called as a statement when its result is intentionally ignored. A no-return
function is called only as a statement.

## Local Scope

Parameters, local `const` bindings, local `var` bindings, loop variables,
`if const` bindings, and catch bindings have lexical block scope. A name cannot
be declared twice in one scope. An inner block may shadow an outer local name.

Top-level constants are compile-time constant expressions over literals, other
constants, field access, operators, interpolation, and range shapes. Calls are
not constant expressions, including constructor and conversion calls. Constants
do not perform durable reads or host operations.

## Effects

The function syntax does not divide calls into separate function and procedure
categories. A function body may read or write durable places, throw, print, or
call a host-provided standard-library function. The checker records those
effects for contexts that restrict them, including transaction bodies,
evolution transforms, presence narrowing, and address expressions.

An optional-producing user call is still a valid subject for `if const` or
`??`. Effect restrictions are applied where the expression is used; optionality
alone does not make a call pure or impure.

## Name Resolution

Within a function, local declarations and parameters resolve before module
declarations. Imported module short names and declarations in the current
module share the file namespace. Built-in names resolve when not shadowed by an
allowed local binding.

Module-level declarations cannot redefine built-ins such as `exists`, `print`,
`Id`, or `Error`.
