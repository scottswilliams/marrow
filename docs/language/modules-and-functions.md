# Modules and functions

A module is one source file with a name. A function takes its arguments by value and may read or write durable places.

A shelf module and a program that imports it:

```text
// src/shelf/books.mw
module shelf::books

resource Book {
    required title: string
}

store ^books[id: int]: Book

pub fn add(id: int, title: string) {
    transaction {
        ^books[id] = Book(title: title)
    }
}

pub fn title(id: int): string? {
    return ^books[id].title
}

// src/main.mw
module main

use shelf::books

pub fn label(id: int): string {
    return books::title(id) ?? "(no book)"
}

test "label reads through the import" {
    books::add(1, "Small Gods")
    assert label(1) == "Small Gods"
    assert label(2) == "(no book)"
}
```

`module shelf::books` names the file at `src/shelf/books.mw`. `use shelf::books` lets `main` call its public functions as `books::add` and `books::title`. `label` reads through the import and takes a default with `??`. The test drives both exports and passes under `marrow test`.

A function returns one value or nothing:

```mw
module docs::modules::title

fn maybeTitle(show: bool): string? {
    if show {
        return "Small Gods"
    }
    return absent
}

pub fn title(show: bool): string {
    return maybeTitle(show) ?? "(hidden)"
}

test "an optional return takes a default" {
    assert title(true) == "Small Gods"
    assert title(false) == "(hidden)"
}
```

`maybeTitle` returns `string?`, so each path returns a `string` or `absent`. `title` consumes the optional with `??`. `pub` makes `title` an export; `maybeTitle` is visible inside the module only.

## Functions

Parameters are named and typed. An omitted return type means the function returns no value. Every reachable path of a value-returning function returns (`check.type`). A function with no return type is called as a statement. A value-returning function may also be called as a statement when its result is unused.

Project and generic functions take positional arguments. A named argument to a function is a `check.type` error; struct and resource constructors name their fields ([source and syntax](source-and-syntax.md)).

Scalars, structs, resources, lists, and maps are passed by value. A parameter is a constant inside the body. A helper that changes a local resource or collection returns the replacement value:

```mw
module docs::modules::parameters

fn increment(count: int): int {
    return count + 1
}

pub fn twice(): int {
    var count = 0
    count = increment(count)
    count = increment(count)
    return count
}

test "a helper returns the replacement value" {
    assert twice() == 2
}
```

`increment` receives a copy of `count` and returns a new value. `twice` assigns the result back each time. A durable root or branch is addressed in place; it is walked with a [bounded traversal](traversal-and-indexes.md#bounded-durable-traversal) or copied entry by entry into a local collection.

A module has one function per name (`check.name_conflict`). A function cannot call itself, directly or through other functions (`check.recursion`).

There is one kind of function. Any body may read or write a durable place. A function that writes runs inside a `transaction` block, its own or a caller's; a call outside one is a `check.requires_transaction` error ([errors and transactions](errors-and-transactions.md#transactions)). A handled failure is an ordinary `Result<T, E>` value ([Option and Result](types-and-values.md#option-and-result)).

## Generic functions

A function may take type parameters in angle brackets after its name. Each parameter names a type usable in the signature and in the body's annotations:

```mw
module docs::modules::generics

fn identity<T>(x: T): T {
    return x
}

fn first<T>(xs: List<T>): T? {
    for x in xs {
        return x
    }
    return absent
}

test "type arguments are inferred" {
    assert identity("Small Gods") == "Small Gods"
    assert first(List(3, 4)) ?? 0 == 3
}
```

Type arguments are inferred from the call's arguments. There is no explicit instantiation syntax, and a parameter that no argument determines is a `check.type` error at the call. Each distinct set of type arguments compiles to its own copy of the function. A generic function is not an export: `marrow run` names only functions whose parameter types are concrete.

A bare type parameter is opaque. The body may pass it, return it, bind it, and hold it in a `List` or `Map`, and nothing else. A constraint after the parameter names the operators the body may use:

| Constraint | Operators | Types that satisfy it |
|---|---|---|
| `supports equality` | `==`, `!=` | `int`, `bool`, `string`, `bytes`, `date`, `instant`, `duration`, nominal ints, enums |
| `supports order` | `<`, `<=`, `>`, `>=`, and equality | `int`, `string`, `bytes`, `date`, `instant`, `duration`, nominal ints |

```mw
module docs::modules::constrained

fn firstBigger<T supports order>(xs: List<T>, threshold: T): T? {
    for x in xs {
        if x > threshold {
            return x
        }
    }
    return absent
}

test "a constraint names the operators a body may use" {
    assert firstBigger(List(1, 5, 9), 4) ?? 0 == 5
    assert firstBigger(List("a", "c"), "b") ?? "" == "c"
}
```

The body is checked against its constraints, whether or not the function is called: `==` on an unconstrained parameter is a `check.type` error at the operator. Each call then checks the argument type against the constraint, so `firstBigger` over a `List<bool>` is a `check.type` error at the call.

Structs and enums take the same type parameters and the same constraints ([generic types](types-and-values.md#generic-types)). Resources and store roots are not generic, and neither a resource nor an entry identity can be a type argument. A generic function that calls itself at an ever-larger type, such as `grow(xs)` with `xs: List<T>` inside `grow<T>`, has no finite set of copies; the compiler stops at a fixed bound and reports `check.instantiation_limit`.

## Modules and imports

A module's name is its file path under `src`, with `::` for `/` and no extension: `src/shelf/books.mw` declares `module shelf::books`. A header that names another path is a `check.module_path` error.

`use` imports a module path and binds its final segment as the local module name. Afterwards `books::add(...)` calls `shelf::books::add`. `use` is optional: the full path `shelf::books::add(...)` is valid in every module. `use` shortens function paths only; types need no import. There are no wildcard or explicitly renamed imports. Two imports with the same final segment are a `check.import` error.

A file without a `module` header is a script. It is checked under its path-derived name and cannot be imported.

On the command line an export is named with dots: `marrow run shelf.books.add` runs `shelf::books::add`, and a script's exports are named the same way. Running an export that touches a store needs a store and the companion layout ([install](../install.md#running-against-a-store)).

## Visibility

`pub fn` is callable from every module and from the command line. A function without `pub` is callable inside its own module; a call from another module is a `check.visibility` error. A top-level constant is visible inside its own module.

Types are project-wide. A resource, struct, or enum declared in any module is used by its bare name everywhere, and two modules cannot declare the same type name. `pub` applies to functions only. A store root is likewise project-wide: any module may read or write `^books`, and `marrow check` reports which exports do.

## Constants

A top-level `const` binds one scalar value for the whole module:

```mw
module docs::modules::constants

const shelfCapacity = 12

pub fn hasRoom(count: int): bool {
    return count < shelfCapacity
}

test "a constant folds into its uses" {
    assert hasRoom(11)
    assert not hasRoom(12)
}
```

The value is an `int`, `bool`, or `string` literal, or a negated integer literal, and it is folded into every use. A type annotation names that scalar type, or an alias of it; a mismatch is a `check.type` error. An expression, a call, or a `bytes` or temporal value in a constant is a `check.unsupported` error.

## Scope and names

Parameters, `const` and `var` bindings, loop variables, and `if const` bindings are visible to the end of their block. An inner block may declare a name that an outer block already holds. A name is declared once per block.

Inside a function, a local name resolves before a module declaration of the same name. A module cannot declare a [reserved built-in](builtins.md#collections) such as `exists`, `List`, or `trim` (`check.name_conflict`). `append` and `length` are ordinary names; a module that declares one shadows the built-in throughout that module.
