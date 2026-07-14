# Implementation guide

This guide maps the code that exists at the current revision. It is descriptive
and intentionally shorter than the source: public language behavior belongs in
the [language reference](../language/), not here.

The beta line began at lane B00 with a deliberate capability trough: the
prototype's entangled compiler, interpreter, catalog, and durable owners were
deleted. A storeless compiler, program image, independent verifier, stack VM,
and a stub path kernel over the retained ordered-byte engine have since been
refounded, so a small typed program now travels source to a reproducible image,
through the verifier and VM, to durable data and back. Lifecycle (evolution,
backup, restore), packages, and served execution are still being refounded lane
by lane; [Project status](../status.md) states what is current and what returns
through which direction.

## Crates

| Crate | Current responsibility | Read next |
|---|---|---|
| `marrow` | CLI: `init`, `fmt`, and `run` (compile, verify, execute an export), plus typed not-yet-supported responses for refounding command names | — |
| `marrow-codes` | Typed diagnostic-code registry and generated code reference | — |
| `marrow-project` | Pure project-input owner: manifest schema, contained discovery, immutable captured input | — |
| `marrow-syntax` | Lexer, parser, AST, formatter, and source diagnostics | [Syntax](syntax.md) |
| `marrow-compile` | Storeless subset checker, language scalar vocabulary, and lowering to a program-image draft | — |
| `marrow-image` | Canonical program-image container, typed draft, encoder, and image-id digest | — |
| `marrow-verify` | The only image decoder and the phased verifier producing the sealed `VerifiedImage` | — |
| `marrow-vm` | Stack VM over the sealed instruction tape, with source-mapped runtime faults | — |
| `marrow-kernel` | Stub path kernel: durable operation algebra, authority triple, store profile, commit witness, and runtime logical codecs | [Storage](storage.md) |
| `marrow-store` | Ordered-byte engine contract, memory and redb backends, engine conformance suite | [Storage](storage.md) |

## Guides

- [Syntax](syntax.md)
- [Storage](storage.md)
- [Testing](testing.md)

## Ownership rule

One typed owner defines each semantic fact. Downstream crates consume stable
typed projections rather than matching source spellings, diagnostic prose,
raw paths, or serialized messages. When a needed fact is missing, add it to the
upstream owner; do not reconstruct it in the CLI, LSP, or tests.
