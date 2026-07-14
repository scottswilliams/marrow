# Marrow documentation

Marrow is an experimental statically typed language being developed as a
general-purpose language with direct durable hierarchical state. These
documents describe the source tree at the same revision; Marrow has not made a
stable release.

## Start here

- [Install Marrow](install.md) from the source tree.
- [Quickstart](quickstart.md) creates, checks, runs, tests, and inspects a
  durable program.
- [Language tour](language/) introduces the programming model in one page.
- [Project status](status.md) separates current behavior, legacy code, and
  future direction.

## Language reference

The [language reference](language/) defines current `.mw` syntax and semantics.
It is organized for lookup rather than sequential reading:

- [Source and syntax](language/source-and-syntax.md)
- [Types and values](language/types-and-values.md)
- [Modules and functions](language/modules-and-functions.md)
- [Resources](language/resources.md)
- [Durable places](language/durable-places.md)
- [Traversal and indexes](language/traversal-and-indexes.md)
- [Control flow](language/control-flow.md)
- [Errors and transactions](language/errors-and-transactions.md)
- [Evolution declarations](language/evolution.md)
- [Builtins](language/builtins.md) and the
  [standard library](language/standard-library.md)
- [Execution limits](language/execution-limits.md) and the
  [grammar](language/grammar.md)

## Tools and operations

- [Tool reference](tools/) covers the project file, CLI, data inspection,
  evolution, backup, restore, and diagnostics.
- [Operations](operations/) covers native-store ownership and recovery.
- [Error codes](error-codes.md) is generated from the current toolchain registry.
- [Compatibility](compatibility.md) states what an unreleased revision does
  and does not promise.
- [Security policy](../SECURITY.md) gives the private reporting channel and
  current support boundary.

## Project and implementation

- [Vision](vision.md) explains the product and architectural direction.
- [Implementation guide](implementation/) maps current crates, owners, and
  code paths for contributors.
- [Contributing](../CONTRIBUTING.md) gives focused and workspace verification
  commands.
- [Future direction](future/) collects unimplemented ideas. Future pages are
  neither current behavior nor implementation prerequisites.

## Documentation authority

The current reference, tests, and implementation change together. One page
owns each public rule; guides link to it instead of copying it. Implementation
pages describe code rather than redefining language behavior. Future pages do
not override either.

Plans, research reports, issue discussions, and old proposals are not language
authority. When documentation and reachable behavior disagree, that mismatch
is a defect to resolve in the reference, tests, or code.

Every `mw` code fence in current documentation is a complete module checked by
the production checker. Short syntax fragments use `text` or `ebnf` fences so
they cannot be mistaken for verified programs. Future pages contain no `mw`
fences because unimplemented syntax is not a reference.
