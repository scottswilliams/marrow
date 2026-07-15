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
| `marrow` | CLI: `init`, `fmt`, `run` (compile, verify, execute a storeless export), `test` (compile a verified test image and run each `test`, driving a durable test against its own fresh ephemeral attachment), and `client typescript` (the deterministic strict-TypeScript generator emitting the pinned Node supervision module beside the generated client), plus typed not-yet-supported responses for refounding command names | — |
| `marrow-codes` | Typed diagnostic-code registry and generated code reference | — |
| `marrow-project` | Pure project-input owner: manifest schema, contained discovery, the `marrow.ids` durable-identity ledger, immutable captured input | — |
| `marrow-syntax` | Lexer, parser, AST, formatter, and source diagnostics | [Syntax](syntax.md) |
| `marrow-compile` | Storeless subset checker, language scalar vocabulary, and lowering to a program-image draft: it emits the whole durable graph's operation sites and lowers named `place` bindings with a once-evaluated key tuple and structured present-entry analysis | — |
| `marrow-image` | Canonical program-image container, typed draft, encoder, and image-id digest; a durable graph node's derived semantic path and a durable operation site's closed whole-payload/field-leaf target; the durable access-demand model (operation class, atom, and the `DemandSetId` demand-set identity); the deployment-ceiling descriptor whose read/write coverage and 32-byte ceiling-id are both derived from a demand union, binding a ceiling to its verified image; and the host-neutral wire interface descriptor, its closed transfer graph, and the `InterfaceId` interface identity | — |
| `marrow-verify` | The only image decoder and the phased verifier producing the sealed `VerifiedImage`; it resolves each operation site's semantic path against its own reconstructed node set and re-derives the site rather than trusting a compiler-side summary, and reconstructs each export's access demand from the sealed sites its call closure reaches | — |
| `marrow-vm` | Stack VM over the sealed instruction tape, with source-mapped runtime faults; the ephemeral-attachment executor derives the flat store schema and site table from a verified image and runs a durable test against a freshly minted attachment | — |
| `marrow-kernel` | Path kernel: durable operation algebra, authority triple, store profile, commit witness, runtime logical codecs, and the production ephemeral-memory attachment (nonforgeable attachment id, deployment ceiling, per-invocation grant) minted from a verified image | [Storage](storage.md) |
| `marrow-store` | Ordered-byte engine contract, memory and redb backends, engine conformance suite | [Storage](storage.md) |
| `marrow-local-wire` | Pure single owner of the local wire: length-prefixed framing with a maximum size, the protocol version byte, canonical JSON (its own value model, codec, and depth/string bounds), the closed handshake/request/response/fault grammar, and the closed loss classification; depends only on `marrow-codes` | — |
| `marrow-runner` | Storeless export runner (library plus stock binary): the supervised Unix-domain channel discipline (mode-0700 dir, listener-before-handshake, launch nonce, poll-based deadlines, explicit fail-closed teardown, bounded loop-accept-until-authenticated), the transfer codec between wire JSON and runtime values, and export dispatch that rejects unknown, durable, and mismatched requests; consumes `marrow-verify` and `marrow-vm` | — |

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
`OperationClass` (`read`, `write`, `presence`, `erase`, `iterate`) projected
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

## Wire interface

A program's wire interface is the set of its concrete root-package exports, each
described host-neutrally by a function descriptor that both real callers — the
terminal and the generated TypeScript client — consume without reparsing source.
`marrow-image` owns the interface vocabulary: a `TransferType` (the closed set of
value types a signature may carry — `unit`, the seven scalars, a `Product`
record, and a `Sum` enum, the last also covering `Option`/`Result`), a
`FunctionDescriptor` pairing an export's `ExportId` with its transfer-projected
parameters and return and its `DemandSetId`, an `Interface` (the descriptor set
sorted by export id), and the `InterfaceId` interface identity — a
domain-separated hash over the sorted descriptor set, distinct from every export
id, every demand-set id, and the image id.

Like access demand, the interface is a fact reconstructed from the verified
image, not a section written into it. Every input — export ids, function
parameter and return types, the record and enum tables, and each export's
reconstructed `DemandSetId` — is already present in a `VerifiedImage`, so the
`InterfaceId` derives from verified facts rather than trusting a compiler-written
summary. A body edit that changes no signature and no demand leaves the
`InterfaceId` fixed while the image id moves; any signature change (a parameter or
return type, a record field name, an enum variant) or demand change moves it.
Finite collections (`List`/`Map`) are deliberately outside the transfer graph at
this stage: a signature that reaches one — directly or through a record field or
enum payload — is rejected with a typed exclusion rather than surfaced on the
wire, and collections join the transfer graph only when the client earns them.
Because a record field or enum payload may itself be a record or enum, each
signature is expanded structurally under a fixed node budget, so a
verified-but-adversarial diamond of many-fielded records cannot drive an
exponential expansion.
