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
use shelf::books

fn example()
    const id = books::add("Small Gods")
    const due = addDays(date("2026-07-15"), 10)
```

The import does not place individual declarations into the local namespace.
Fully qualified calls such as `shelf::books::titleOf(id)` remain valid. There are no
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

On the command line an export is addressed by its dot-separated declaration
path — `marrow run shelf.books.add` for the path source spells
`shelf::books::add` — and a headerless script's exports stay addressable by its
path-derived name even though the script is not importable.

## Declarations And Returns

```mw
module docs::functions

fn maybeTitle(show: bool): string? {
    if show { return "Marrow" }
    return absent
}

pub fn title(show: bool): string {
    return maybeTitle(show) ?? "(hidden)"
}
```

Parameters are named and typed. An omitted return type means the function
returns no value. Every reachable path of a value-returning function must
return. An optional return `T?` may return either `T` or `absent`.

Marrow does not overload functions. A module has at most one function with a
given name. Recursion is not admitted: a function may not call itself, directly
or through a cycle of other functions; the direct-call graph is acyclic.

A function may not reuse a reserved built-in name (`List`, `Map`, the text floor,
and the value constructors); the common collection verbs `append`, `insert`,
`get`, and `length` are not reserved, so declaring one shadows the corresponding
built-in throughout that module (see [Built-ins](builtins.md#collections)).

## Generic Functions

A function may declare rank-1 generic type parameters in a bracket list after its
name, written with the same bracket convention type applications use
(`List<T>`). Each parameter names a type usable in the parameter, return, and
local annotations of the body.

```mw
module docs::generics

fn identity<T>(x: T): T {
    return x
}

fn first<T>(xs: List<T>): T? {
    for x in xs {
        return x
    }
    return absent
}
```

Type arguments are inferred from the call's arguments; there is no explicit
instantiation syntax. A type parameter that no argument determines cannot be
inferred and is a `check.type` error at the call site. Each distinct set of
inferred type arguments produces one monomorphized instance; instances are
internal image functions with no stable identity, and a generic function is not
itself an invocable export.

A bare type parameter is opaque: the body may pass it, return it, bind it, and
store it in a `List`/`Map`, but it admits no operators of its own. A parameter may
carry one closed constraint that licenses a family of operators over it:

| Constraint | Licenses | Concrete types that satisfy it |
|---|---|---|
| `supports equality` | `==` and `!=` | `int`, `bool`, `string`, `bytes`, nominal types, enums |
| `supports order` | `<`, `<=`, `>`, `>=` (and equality) | `int`, `string`, `bytes`, nominal types |

```mw
module docs::constrained

fn includes<T supports equality>(xs: List<T>, target: T): bool {
    for x in xs {
        if x == target { return true }
    }
    return false
}

fn firstBigger<T supports order>(xs: List<T>, threshold: T): T? {
    for x in xs {
        if x > threshold { return x }
    }
    return absent
}
```

A generic body is checked once against its parameters' constraints, so using an
operator a parameter does not support — `==` on an unconstrained parameter, or
`<` on one constrained only by equality — is a `check.type` error whether or not
the function is ever called. Each application then revalidates that the concrete
type actually supports the constraint, so instantiating an order-constrained
parameter with a type that has no order (such as `bool`) is a `check.type` error
at the call site.

Type parameters also apply to `struct` and `enum` value types (see
[Generic types](types-and-values.md#generic-types)); a function and a value type
share one monomorphization mechanism and the same `supports` constraints. There are
no generic resources, places, or host imports, and a type parameter may not be a
`Map` key. Because the call and value-containment graphs are acyclic,
monomorphization always terminates; a bound (`check.instantiation_limit`) fails a
program whose monomorphization would otherwise diverge.

## Parameters Are By Value

Scalars, structs, resources, lists, and maps are passed by value. Parameters
cannot be assigned. A helper that changes a local resource or collection returns
the replacement value.

```mw
module docs::parameters

fn increment(count: int): int {
    return count + 1
}

pub fn example(): int {
    var count = 0
    count = increment(count)
    count = increment(count)
    return count
}
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

A top-level constant binds a scalar value and is module-private: it is referred
to by name only within its own module, and it is folded into its uses at compile
time. Its value is a scalar literal (`int`, `bool`, or `string`) or a negated
integer literal, and an optional type annotation must name that scalar type. A
constant performs no durable read, call, or host operation. Richer constant
expressions over other constants, operators, interpolation, and range shapes are
a later addition.

## Effects

The function syntax does not divide calls into separate function and procedure
categories. A function body may read or write durable places, throw, print, or
call a host-provided standard-library function. The checker records those
effects for contexts that restrict them, including transaction bodies,
presence narrowing, and address expressions.

An optional-producing user call is still a valid subject for `if const` or
`??`. Effect restrictions are applied where the expression is used; optionality
alone does not make a call pure or impure.

A function that mutates durable state carries a checked *requires an ambient
transaction* effect: it is callable only inside a `transaction` block or from a
function that carries the effect in turn. Calling such a function, or performing
a durable mutation, in an export body with no enclosing `transaction` is a
`check.requires_transaction` error. See
[Errors and transactions](errors-and-transactions.md#transactions).

## Name Resolution

Within a function, local declarations and parameters resolve before module
declarations. Imported module short names and declarations in the current
module share the file namespace. Built-in names resolve when not shadowed by an
allowed local binding.

Module-level declarations cannot redefine built-ins such as `exists`, `print`,
`Id`, or `Error`.
