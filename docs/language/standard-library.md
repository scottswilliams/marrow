# Standard Library

The current toolchain supplies no `std::` modules. A path beginning with `std::`
uses ordinary project-module resolution. An absent module or function reports
`check.type`; a cross-module call to a non-public function reports
`check.visibility`. A project-declared `std::` path is project code, not an
ambient library.

Compiler-owned built-ins and constructors are available without module imports.
[Built-ins](builtins.md) documents the current callable forms and the value
built-ins `maxInt`/`minInt` (the `int` domain bounds), and
[Types and values](types-and-values.md) defines their value boundaries.

A source-defined standard library is future direction recorded in
[Source standard library](../future/source-standard-library.md). That direction
does not make a module or callable form current.
