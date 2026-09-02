# Marrow documentation

Marrow is a statically typed compiled language in which durable data is
ordinary program state. These pages describe the source tree at the
revision that carries them. Marrow is unreleased.

## Reading order

- [Installation](install.md) builds the `marrow` command from source.
- [Quickstart](quickstart.md) goes from `marrow init` to a running durable
  program.
- [Walkthrough](walkthrough.md) reads a complete durable application line by
  line.
- [Language reference](language/) defines current `.mw` syntax and semantics.
  Its chapters build on one another in order, and its appendices are for
  lookup; the first page lists both.
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
