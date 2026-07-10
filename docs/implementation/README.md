# Implementation guide

This guide maps the code that exists at the current revision. It is descriptive
and intentionally shorter than the source: public language behavior belongs in
the [language reference](../language/), not here.

## Pipeline

```text
.mw source
  -> syntax AST and diagnostics
  -> schema shapes
  -> checked semantic facts and executable representation
  -> catalog attachment and execution fence
  -> tree-walking runtime
  -> managed logical tree writes
  -> memory or redb substrate
```

`marrow-check` is the current semantic spine. The runtime, CLI, JSON adapters,
and downstream language server consume its typed facts rather than reparsing
source meaning.

## Crates

| Crate | Current responsibility | Read next |
|---|---|---|
| `marrow` | CLI dispatch and command processes | [Tooling](tooling.md) |
| `marrow-catalog` | Accepted declaration metadata and validation | [Lifecycle](lifecycle.md) |
| `marrow-check` | Resolution, types, presence/effects, evolution discharge, lowering, analysis facts | [Compiler](compiler.md) |
| `marrow-codes` | Typed diagnostic-code registry and generated code reference | [Tooling](tooling.md) |
| `marrow-json` | Structured CLI, data, and legacy surface DTOs | [Tooling](tooling.md) |
| `marrow-project` | `marrow.json`, discovery, and project digest | [Tooling](tooling.md) |
| `marrow-run` | Interpreter, managed writes, sessions, and evolution apply | [Runtime](runtime.md) |
| `marrow-schema` | Resource, store, enum, builtin, and standard-library shapes | [Compiler](compiler.md) |
| `marrow-store` | Logical cells, transactions, metadata, backup framing, memory/redb backends | [Storage](storage.md) |
| `marrow-syntax` | Lexer, parser, AST, formatter, and source diagnostics | [Syntax](syntax.md) |

## Guides

- [Syntax](syntax.md)
- [Compiler](compiler.md)
- [Runtime](runtime.md)
- [Storage](storage.md)
- [Lifecycle](lifecycle.md)
- [Tooling](tooling.md)
- [Testing](testing.md)
- [Legacy implementation](legacy.md)

## Ownership rule

One typed owner defines each semantic fact. Downstream crates consume stable
typed projections rather than matching source spellings, diagnostic prose,
raw paths, or serialized messages. When a needed fact is missing, add it to the
upstream owner; do not reconstruct it in the CLI, runtime, LSP, or tests.
