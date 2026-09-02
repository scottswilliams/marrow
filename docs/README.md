# Marrow documentation

Marrow is a statically typed compiled language in which durable data is
ordinary program state. These pages describe the source tree at the same
revision; Marrow is unreleased.

## Reading order

- [Installation](install.md) builds the `marrow` command from source.
- [Quickstart](quickstart.md) goes from `marrow init` to a running durable
  program.
- [Walkthrough](walkthrough.md) reads a complete durable application line by
  line.
- [Language reference](language/) defines current `.mw` syntax and semantics.
  Its chapters run in order from source and syntax through types and values,
  modules and functions, control flow, resources, durable places, errors and
  transactions, traversal and indexes, tests, and idioms, with builtins,
  execution limits, the grammar, and a sample as appendices.
- [Vision](vision.md) states the product direction and its boundaries.

## Reference

- [Tool reference](tools/) covers the `marrow` command, projects, tests, the
  `marrow-lsp` editor server, the TypeScript client, and machine-readable
  language facts.
- [Operations](operations/) covers a store on disk: provisioning, running an
  export against it, changing the program, and interrupted commits.
- [Error codes](error-codes.md) lists every diagnostic, fault, and operational
  code; the page is generated from the toolchain.
- [Compatibility](compatibility.md) states what an unreleased revision promises.
- [Project status](status.md) separates current behavior from future direction
  and records measurements.
- [Implementation guide](implementation/) maps the Rust crates for contributors.
- [Future direction](future/) records unimplemented direction.

The language reference defines current behavior. One page owns each public
rule; guides link to it instead of copying it. Every `mw` fence in current
documentation is a complete source file that compiles and passes `marrow test`;
fragments use `text` fences. A future page defines no syntax.
[Contributing](../CONTRIBUTING.md) states the documentation rules and the
checks; the [security policy](../SECURITY.md) gives the reporting channel.
