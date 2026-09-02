# Source standard library

Portable library behavior is ordinary Marrow source, compiled and verified
with the applications that use it.

## Today

The current toolchain supplies no `std::` modules. A project-declared `std::`
path follows ordinary project-module resolution and remains project code. The
built-in functions are the whole ambient vocabulary
([builtins](../language/builtins.md#no-standard-library)).

## Direction

The first source-defined layer holds `Option` and `Result` helpers, generic
collection combinators, bounded text utilities, comparison helpers, and other
pure behavior that needs no privileged runtime state. It ships as a package
whose lineage is toolchain-pinned, so source spelling cannot impersonate it
([packages](packages.md)).

Library code uses the same generics, image, verifier, and VM as application
code, and only the implemented procedural floor; it needs no closures. A VM
intrinsic owns only an operation that source cannot express portably or within
its measured bounds. Library code cannot recompute what the compiler knows, and
project code is not a compiler plugin.

The compiler, verifier, VM, package acquirer, engine, and lifecycle stay in
Rust. Applications and the library are Marrow.

## Evidence

The library builds as an ordinary package, and Graph Report and Club Locker
use it through the package system ([local applications](local-applications.md)).
