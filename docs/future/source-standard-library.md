# Source standard library

Portable library behavior belongs in ordinary Marrow source, compiled and
verified with its caller.

## Today

The toolchain supplies no `std::` modules. A project-declared `std::` path is
ordinary project code. [Builtins](../language/builtins.md#no-standard-library)
define the current ambient vocabulary.

## Direction

Extract a helper when maintained programs share real behavior. Use the same
generics, value types and runtime as application code. An intrinsic is justified
only when source cannot express an operation portably or within measured bounds.

The beta needs [source reuse](packages.md), not a standard-library portfolio,
toolchain-pinned package lineage, combinator framework or new privileged
namespace. Broader library organization is deferred until actual callers show
what it must contain.

## Evidence

Two maintained callers reuse a source helper with fewer duplicated rules and
unchanged behavior. The helper compiles and tests through ordinary tooling, with
no privileged initialization or host authority.
