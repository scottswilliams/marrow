# Marrow Language Reference

This directory describes the Marrow language implemented by the current parser,
checker, compiler, and stack virtual machine. A Marrow program combines ordinary local
values with typed durable places. The `^` prefix distinguishes a durable place
from a local value; reading, assigning, deleting, and traversing either form use
the same expression and statement vocabulary.

The reference describes observable source behavior. Command-line workflows,
project configuration, native-store operation, and implementation structure are
documented elsewhere.

## First Look

This complete module declares a resource shape, a keyed durable root, and two
functions that use it.

```mw
module app::tasks

resource Task {
    required title: string
    done: bool
}

store ^tasks[id: int]: Task

pub fn add(id: Id(^tasks), title: string): Id(^tasks) {
    ^tasks[id].title = title
    return id
}

pub fn complete(id: Id(^tasks)): bool {
    if not exists(^tasks[id]) { return false }

    ^tasks[id].done = true
    return true
}
```

`Task.title` is required. Creating an entry by field assignment therefore
requires `title` to be present at the end of the current write or outer
transaction. `done` is sparse: it is absent until assigned. `add` takes the
entry identity as an `Id(^tasks)` parameter: the caller supplies the identity
rather than the store minting one.

## Reading The Reference

The pages are arranged from source text to durable behavior:

- [Source and syntax](source-and-syntax.md) defines files, tokens, literals,
  declarations, statements, expressions, and paths.
- [Types and values](types-and-values.md) defines scalar, resource, collection,
  identity, optional, and error values.
- [Modules and functions](modules-and-functions.md) defines name resolution,
  visibility, calls, parameters, and returns.
- [Resources](resources.md) defines sparse records, required members, keyed
  layers, and local construction.
- [Durable places](durable-places.md) defines stores, identities, presence,
  reads, writes, clearing, and deletion.
- [Traversal and indexes](traversal-and-indexes.md) defines ordered iteration,
  ranges, positional keyed leaves, and maintained alternate orderings.
- [Control flow](control-flow.md) defines branches, loops, matching, and
  evaluation order.
- [Errors and transactions](errors-and-transactions.md) defines thrown values,
  catchable faults, commit, and rollback.
- [Tests](tests.md) defines the `test` declaration and the owned `assert`
  statement that `marrow test` runs.
- [Built-ins](builtins.md) and the
  [standard library](standard-library.md) list callable operations.
- [Execution limits](execution-limits.md) records fixed runtime and parser
  bounds.
- [Grammar](grammar.md) gives a syntax-only EBNF summary.
- [The surface laws](surface-laws.md) state the closed sigil economy, the grep
  contract, and the no-synonym law.
- [Idioms](idioms.md) describes how idiomatic Marrow is written.
- [Reference sample](sample.md) is a larger checked and executed module.

## Core Terms

**Place**
: A location that can be read and may be assignable. Local variables, local
  resource members, local collection elements, and durable paths are places.

**Resource**
: A declared hierarchical value shape. Fields are sparse unless marked
  `required`; keyed child declarations add typed child layers.

**Durable place**
: A path beginning with a declared `^store`. Its value persists through the
  configured store used by a normal run.

**Entry identity**
: A value of type `Id(^root)`. It identifies one entry of a particular store
  root and is not interchangeable with the identity of another root.

**Presence**
: Whether a value exists at a path. An optional type `T?` represents either a
  present `T` or `absent`.

**Transaction**
: A block whose durable changes commit together or roll back together.

## Observable Execution Model

The checker resolves names and types and the compiler lowers the program to a
verified image; the current stack virtual machine then executes it. Expressions
evaluate from left to right, with
short-circuit behavior for `and`, `or`, and `??`. Durable reads inside a
transaction observe staged changes from that transaction.

A project's `test` declarations run storeless through `marrow test`: each is a
named zero-argument body whose owned `assert` statement checks a condition. See
[Tests](tests.md).

## Current Scope

The reference covers the current non-legacy `.mw` language and behavior
available through the checker and runtime. Programs use typed paths, ordinary
control flow, and explicit subtree traversal. The prototype `surface` syntax
was deleted at B00; see [Project status](../status.md).
