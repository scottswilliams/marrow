# Implementation guide

This guide maps the code that exists at the current revision. It is descriptive
and intentionally shorter than the source: public language behavior belongs in
the [language reference](../language/), not here.

The beta line began at lane B00 with a deliberate capability trough: the
prototype's compiler, interpreter, catalog, and durable owners were deleted,
and the workspace was reduced to the four retained crates below. The compiler,
program image, verifier, VM, path kernel, and durable lifecycle are being
refounded lane by lane; [Project status](../status.md) states what is current
and what returns through which direction.

## Crates

| Crate | Current responsibility | Read next |
|---|---|---|
| `marrow` | Thin CLI: `fmt` over a single file, plus typed not-yet-supported responses for refounding command names | — |
| `marrow-codes` | Typed diagnostic-code registry and generated code reference | — |
| `marrow-store` | Ordered-byte engine contract, memory and redb backends, engine conformance suite | [Storage](storage.md) |
| `marrow-syntax` | Lexer, parser, AST, formatter, and source diagnostics | [Syntax](syntax.md) |

## Guides

- [Syntax](syntax.md)
- [Storage](storage.md)
- [Testing](testing.md)

## Ownership rule

One typed owner defines each semantic fact. Downstream crates consume stable
typed projections rather than matching source spellings, diagnostic prose,
raw paths, or serialized messages. When a needed fact is missing, add it to the
upstream owner; do not reconstruct it in the CLI, LSP, or tests.
