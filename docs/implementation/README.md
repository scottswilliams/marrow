# Implementation guide

Marrow's workspace separates syntax, compilation, verification, execution,
storage, and tooling into crates. The
[language reference](../language/) states what a program means; this guide
states where that meaning is computed.

## Pipeline

A program travels one way. `marrow-syntax` parses `.mw` source into an AST.
`marrow-compile` checks the AST and lowers it to a program-image draft, which
`marrow-image` encodes to canonical bytes. `marrow-verify` is the only decoder:
it rebuilds every executable claim from the bytes and seals a `VerifiedImage`.
`marrow-vm` runs a sealed image over its instruction tape. Durable reads and
writes leave the VM through `marrow-kernel`, which encodes keys and values and
drives a transaction against an engine in `marrow-store`.

The compiler opens no store, and the VM accepts only an image the verifier
sealed. A store is provisioned, opened, and imported through `marrow-lifecycle`,
whose file operations go through `marrow-fs-journal`. The same crate stack
serves a durable `test` by minting a fresh in-memory store from the verified
image and discarding it when the test ends.

The compiler retains parser syntax. Its private `types/aliases.rs` owner stores
each supported alias as a shared global terminal name and optionality. It
normalizes chains iteratively and refuses unsupported target shapes before
dependent fills. Type consumers resolve written parameters before aliases and
carry existing declaration refusals through scalar and value-type checks;
they do not allocate expanded alias trees.

Each completed function moves its instruction allocation into the image draft.
The compiler retains the append-returned function identity and full source
coordinates. After body settlement, transaction validation borrows the draft's
instructions and checks that the coordinates cover exactly that sequence.
Template proofs use the same append path and erase their additions on completion.

The draft keeps a saturating charge of the bytes its retained bodies alone commit
the image to (one byte per instruction plus one span row per span), snapshotted and
restored with its transactions. After each settled body the compiler polls it; once
the charge exceeds the image byte ceiling, both compilation and editor analysis stop
lowering and report the `ImageBytes` resource limit without a snapshot. An invariant
discovered in executed work is reported ahead of that stop and of parse or
structural findings. The retained population at a stop is bounded by the largest
prefix under the charge plus one body; this is a retention bound, not a capacity
claim.

A tool sees a project through two layers. `marrow-project` is pure: manifest,
module discovery, and the `.marrow/ids` ledger, all over bytes a caller supplies.
`marrow-project-fs` reads those bytes from disk under fixed bounds and publishes
the ledger. Both the CLI and the language server enter through `marrow-project-fs`.

## Crates

| Crate | Owns | Read next |
|---|---|---|
| `marrow` | The CLI: `init`, `fmt`, `check`, `run`, `test`, `import`, `image`, and `client typescript` | [CLI](../tools/cli.md) |
| `marrow-codes` | The diagnostic-code registry and the generated [error-code reference](../error-codes.md) | [Diagnostic voice](diagnostic-voice.md) |
| `marrow-syntax` | Lexer, parser, AST, formatter, and the diagnostic types every crate renders | [Syntax](syntax.md) |
| `marrow-temporal` | The `date`, `instant`, and `duration` domain: calendar, range, canonical text, and arithmetic. Depends on nothing else in the workspace | [Types and values](../language/types-and-values.md) |
| `marrow-compile` | The checker, the scalar vocabulary, lowering to the image draft, and the `AnalysisSnapshot` the language server reads | [Diagnostic voice](diagnostic-voice.md) |
| `marrow-image` | The program-image container, the validating `ImageDraft`, the canonical encoder, and the `ImageId` digest. Holds no decoder | [Compiled programs](../future/compiled-programs.md) |
| `marrow-verify` | The only image decoder and the phased verifier that seals a `VerifiedImage`; rebuilds each export's durable access demand from the image alone | [Trust boundaries](../status.md#trust-boundaries) |
| `marrow-vm` | The stack VM over a sealed image: source-mapped runtime faults, execution bounds, and the executor that runs a durable test against a fresh in-memory store | [Execution limits](../language/execution-limits.md) |
| `marrow-kernel` | The path over which every durable read and write passes: key and value codecs, the operation algebra, the transaction commit witness, and commit recovery | [Storage](storage.md) |
| `marrow-store` | The ordered-byte engine contract, the in-memory and redb engines, and the conformance suite both must pass | [Storage](storage.md) |
| `marrow-lifecycle` | Provision, open, and import of a persistent store: store identity, envelope, active head, and recovery after an interrupted commit | [Operations](../operations/README.md) |
| `marrow-fs-journal` | Descriptor-rooted file publication: entry-name admission, the cooperative lock, and the pending-journal frame with replay and crash-debris classification | [Storage](storage.md) |
| `marrow-project` | Manifest schema, module discovery, file identities, and the `.marrow/ids` ledger, all over caller-supplied bytes | [Projects](../tools/projects.md) |
| `marrow-project-fs` | Bounded reads of the project root, manifest, source tree, and ledger, and the sole publisher of `.marrow/ids` | [Projects](../tools/projects.md) |
| `marrow-local-wire` | The framed protocol between a runner and its client: framing, limits, canonical JSON, and the closed request, response, fault, and incomplete grammar | [TypeScript client](../tools/typescript-client.md) |
| `marrow-runner` | The runner binary and library: the supervised Unix-domain channel, export dispatch over a verified image, and classification of an outcome the client could not confirm | [Interrupted commits](../operations/README.md#interrupted-commits) |
| `marrow-lsp` | The standalone `marrow-lsp` executable: JSON-RPC over stdio, document sync, and diagnostics, formatting, hover, definition, completion, signature help, and document symbols projected from the compiler's `AnalysisSnapshot` | [Language server](../tools/lsp.md) |

The language server is its own executable. The `marrow` CLI has no `lsp`
subcommand.

## Dependency direction

Every dependency points at a lower level. A crate names only crates beneath
it, so a change in a leaf rebuilds the leaf and its consumers and nothing else.

```text
marrow (CLI)        marrow-lsp
marrow-runner       marrow-project-fs
marrow-lifecycle    marrow-local-wire    marrow-compile
marrow-vm           marrow-verify        marrow-project
marrow-kernel       marrow-image         marrow-syntax
marrow-store        marrow-fs-journal    marrow-codes       marrow-temporal
```

Four leaves have no workspace dependency at all: `marrow-codes`,
`marrow-temporal`, `marrow-image`, and `marrow-fs-journal`. The compiler
reaches `marrow-image` but never `marrow-verify`, `marrow-vm`, or
`marrow-store`: it can emit bytes and cannot mint a verified image or open a
store. The VM reaches `marrow-kernel` and `marrow-verify` but never
`marrow-compile`: it cannot see source. The language server reaches
`marrow-compile` and `marrow-project-fs` and nothing below the image.

## Tracing a command

`marrow test` shows the whole stack in one invocation. The CLI asks
`marrow-project-fs` to capture the project; `marrow-project` turns the captured
bytes into a `ProjectInput`. `marrow-compile` checks every module and lowers a
test image, which `marrow-image` encodes and `marrow-verify` seals. For each
`test` block, `marrow-vm` runs the body. A body that touches a `^` place runs
against a store `marrow-kernel` mints in memory from the sealed image, over the
in-memory engine in `marrow-store`. The store is dropped when the test returns.

`marrow run <export> --store <dir>` replaces the last step. `marrow-lifecycle`
opens the directory, `marrow-store` takes the engine lock, and `marrow-runner`
dispatches the export over the persistent redb engine.

## Guides

- [Syntax](syntax.md): the lexer, parser, AST, and formatter.
- [Storage](storage.md): the layers between a `^` path and bytes on disk.
- [Testing](testing.md): where each kind of test lives and how the battery runs.
- [Compilation and test speed](speed.md): the three clocks and their baselines.
- [Diagnostic voice](diagnostic-voice.md): how a diagnostic is worded and rendered.

## Ownership rule

The design rule is one typed owner per semantic fact. Downstream crates should
consume typed projections rather than recover meaning from source spellings,
diagnostic prose, raw paths, or serialized messages. Add a missing fact to its
upstream owner and publish it through the appropriate interface.

Current code does not fully satisfy that rule. The compiler's
[`FunctionRegistry`](../../crates/marrow-compile/src/lower/registry.rs) predicts
image function indices, while the
[`ImageDraft`](../../crates/marrow-image/src/draft.rs) assigns them as functions
are added; the two depend on coordinated ordering. These overlapping decisions
remain a simplification target.

Independent verification is a separate trust boundary: the verifier reconstructs
types and demand from image bytes without consulting compiler state. Diagnostic
code spellings live in `marrow-codes`. The language server projects compiler
snapshot facts and owns their protocol representation and document state; a
missing semantic editor fact belongs in `marrow-compile`.

## Artifact fence

Compilation is a chain of phases, and each phase takes a typed proof of the
phase before it. `SignaturesComplete` is the zero-size proof that every declared
signature resolved; `encode` takes that proof, never the resolved registry, so an
unproven registry cannot reach the encoder.

A refusal withholds exactly the artifacts that depend on it and no others. A
signature the checker could not resolve is a refused entry in the declaration
ledger, so every other body still lowers and reports its own errors. The proof
is withheld, and that alone fences the program off from `encode`. No phase runs
because the diagnostic set happens to be empty; each takes its own prerequisite,
and an unavailable artifact never produces a substitute.
