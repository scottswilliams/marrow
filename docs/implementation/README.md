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
| `marrow-project` | Pure project-input owner: manifest schema, contained discovery, the `marrow.ids` durable-identity ledger, immutable captured input | — |
| `marrow-syntax` | Lexer, parser, AST, formatter, and source diagnostics | [Syntax](syntax.md) |
| `marrow-compile` | Storeless subset checker, language scalar vocabulary, and lowering to a program-image draft: it emits the whole durable graph's operation sites and lowers named `place` bindings with a once-evaluated key tuple and structured present-entry analysis | — |
| `marrow-image` | Canonical program-image container, typed draft, encoder, and image-id digest; a durable graph node's derived semantic path and a durable operation site's closed whole-payload/field-leaf target; the durable access-demand model (operation class, atom, and the `DemandSetId` demand-set identity) | — |
| `marrow-verify` | The only image decoder and the phased verifier producing the sealed `VerifiedImage`; it resolves each operation site's semantic path against its own reconstructed node set and re-derives the site rather than trusting a compiler-side summary, and reconstructs each export's access demand from the sealed sites its call closure reaches | — |
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

## Access demand

An export's durable access demand is a compiler fact the verifier reconstructs,
not a serialized summary. `marrow-image` owns the demand vocabulary: an
`OperationClass` (`read`, `write`, `presence`, `erase`, `index-read`) projected
from the durable operation algebra, a `DemandAtom` pairing a semantic path with a
class, an `ExportDemand` (the canonical sorted atom set with a read/write coverage
projection and a program-wide union), and the `DemandSetId` demand-set identity —
a domain-separated hash over the sorted atoms, distinct from the export id and the
image id. `marrow-verify` is the single effects owner: it reconstructs each
function's atom set over its acyclic call closure from the sealed sites its opcodes
reference, projects the mutate/read coverage the transaction lattice and the store
ceiling consume, and records each export's demand, its `DemandSetId`, and its
image-local reachable-site set (never part of any identity). Test entries carry
their own demand in a parallel table. The image serializes none of this; the
verifier rebuilds it from the operation sites and bytecode. The kernel's
authority triple checks that read/write coverage against a deployment ceiling and
an invocation grant — admission uses the program-wide union, an invocation uses
its named export's demand — and demand is never a source of rights.
