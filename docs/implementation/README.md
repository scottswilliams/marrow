# Implementation guide

Marrow's workspace separates syntax, compilation, verification, execution,
storage, and tooling into crates. The
[language reference](../language/) states what a program means; this guide
states where that meaning is computed.

## Pipeline

A program travels one way, each stage handing the next a narrower artifact.
Mechanism lives in the module that owns it; this names the owners.

**Syntax.** `marrow-syntax` parses `.mw` source into a spanned AST and owns the
formatter, the diagnostic types every other crate renders, and the position-bound
[query syntax](syntax.md) editor completion reads.

**Compile.** `marrow-compile` resolves names, checks types and effects, and lowers
the AST into an image draft. One drive serves three projections: `compile` encodes
a program image, `analyze` publishes an `AnalysisSnapshot` for the language server
and never encodes, and `check` collects the complete diagnostic union and encodes
a test-inclusive image without publishing editor facts. The compiler
opens no store and cannot mint a verified image.

[`lower/presence.rs`](../../crates/marrow-compile/src/lower/presence.rs) owns
entry-presence proof lifetimes and guard recognition.
It retains shared call-history paths across diverging arms and loop joins;
[`compile/presence_calls.rs`](../../crates/marrow-compile/src/compile/presence_calls.rs)
checks protected uses against callee erasures in bounded family stripes.
[`lower/durable.rs`](../../crates/marrow-compile/src/lower/durable.rs) lowers
checked entry bindings using that owner. Statement lowering reuses these facts
for conditional blocks, early-return guards and `require`; image
verification independently reconstructs presence from their control flow.

**Image.** `marrow-image` owns the container: the draft that validates as it is
built, the instruction set, the canonical encoder, and the `ImageId` digest — but
no decoder. `IMAGE_FORMAT_VERSION` names the admitted generation and separates
digest domains across them ([compatibility](../compatibility.md#versioning)).

`marrow_image::bounds` holds the representational bounds. A bound is a decode-time
allocation guard, never a stored-format byte — the image encodes actual counts — so
widening one is monotone: every image a narrower bound accepted a wider one still
accepts byte for byte, and an older toolchain meeting a newer image either accepts it
unchanged or refuses it with a typed bound rejection. No container or profile version
bump is required today. That changes once images cross a trust or version boundary
(signed artifacts, cross-node acceptance, a capability-gated profile), where a widen
becomes an acceptance-set change a version or capability descriptor must record.

**Verify.** `marrow-verify` is the only decoder. It rebuilds every executable claim
— types, control flow, transaction structure, durable demand, presence proofs — from
the image bytes alone, without consulting compiler state, and seals a
`VerifiedImage`. A retired encoding rejects here and requires recompilation.

**VM.** `marrow-vm` executes the instruction tape of a function selected from a
sealed image, and owns runtime faults mapped back to source spans, the
[execution limits](../language/execution-limits.md), and the canonical value text
that string conversion and CLI output share.

**Kernel.** Every durable read and write leaves the VM through `marrow-kernel`: key
and value codecs, the operation algebra, the commit witness and its recovery, and the
shared audit/export walk. Application code never receives a physical key or an engine
handle.

**Store.** `marrow-store` owns the ordered-byte engine contract, its in-memory and
redb implementations, and the conformance suite both must pass. Engine names and
formats stay out of `.mw` source and public APIs.

**Lifecycle, runner, and tools.** `marrow-lifecycle` prepares a verified image once
and pairs it with the store it admits it for, so a store runs exactly the image the
lifecycle admitted. `marrow-runner` dispatches an export over that pairing in its own
process; `marrow-lsp` projects compiler snapshot facts and adds no semantics.

## Crates

| Crate | Owns | Read next |
|---|---|---|
| `marrow` | The CLI: `init`, `fmt`, `check`, `run`, `test`, `import`, `doctor`, `apply`, `recover`, `backup`, `restore`, `image`, and `client typescript` | [CLI](../tools/cli.md) |
| `marrow-codes` | The diagnostic-code registry and the generated [error-code reference](../error-codes.md) | [Diagnostic voice](diagnostic-voice.md) |
| `marrow-syntax` | Lexer, parser, AST, formatter, and the diagnostic types every crate renders | [Syntax](syntax.md) |
| `marrow-temporal` | The `date`, `instant`, and `duration` domain: calendar, range, canonical text, and arithmetic. Depends on nothing else in the workspace | [Types and values](../language/types-and-values.md) |
| `marrow-compile` | The checker, the scalar vocabulary, lowering to the image draft, and the `AnalysisSnapshot` the language server reads | [Diagnostic voice](diagnostic-voice.md) |
| `marrow-image` | The program-image container, the validating `ImageDraft`, the canonical encoder, and the `ImageId` digest. Holds no decoder | [Compiled programs](../future/compiled-programs.md) |
| `marrow-verify` | The only image decoder and the phased verifier that seals a `VerifiedImage`; rebuilds each export's durable access demand from the image alone | [Trust boundaries](../status.md#trust-boundaries) |
| `marrow-vm` | The stack VM over a sealed image: source-mapped runtime faults, execution bounds (`value.rs::collection_within_limits`, `Value::structural_bytes`, `run.rs::bounded_list`), and durable execution of an export or a source test through the attachment the lifecycle prepared | [Execution limits](../language/execution-limits.md) |
| `marrow-kernel` | The path over which every durable read and write passes: key and value codecs, the operation algebra, the transaction commit witness, commit recovery, the shared audit/export walk, and consuming private restore construction | [Storage](storage.md) |
| `marrow-store` | The ordered-byte engine contract, the in-memory and redb engines, and the conformance suite both must pass | [Storage](storage.md) |
| `marrow-lifecycle` | The verified image's store projection and its pairing with a native or in-memory store; provision, attach, import, audit, explicit sparse-field apply, recovery, and logical backup/restore. The envelope gates ordinary service on Active | [Operations](../operations/README.md) |
| `marrow-fs-journal` | Descriptor-rooted file publication: entry-name admission, the cooperative lock, and the pending-journal frame with replay and crash-debris classification | [Storage](storage.md) |
| `marrow-project` | Manifest schema including the `[dependencies]` alias and relative-path vocabulary, module discovery, file identities, and the `.marrow/ids` ledger, all over caller-supplied bytes | [Projects](../tools/projects.md) |
| `marrow-project-fs` | Bounded reads of the project root, manifest, source tree, and ledger; locating each declared dependency and capturing its tree beside the root's under one limit accumulator, read-only; and the sole publisher of `.marrow/ids` | [Projects](../tools/projects.md) |
| `marrow-local-wire` | The framed protocol between a runner and its client: framing, limits, the workspace's one canonical JSON writer and lexer, completed bounded frames, and the closed request, response, fault, and incomplete grammar | [TypeScript client](../tools/typescript-client.md) |
| `marrow-runner` | The runner binary and library: the supervised Unix-domain channel, export dispatch over a verified image, and the one-shot provision, import, audit, apply, recovery, backup and restore commands over the lifecycle owners | [Operations](../operations/README.md) |
| `marrow-lsp` | The standalone `marrow-lsp` executable: JSON-RPC over stdio, document sync, and diagnostics, formatting, hover, definition, completion, signature help, and document symbols projected from the compiler's `AnalysisSnapshot` | [Language server](../tools/lsp.md) |
| `marrow-test-support` | Shared scratch directories, captured project inputs, ledger fixtures, owned-heap ceiling, image draft seam and forger, engine doubles, and verifier corpus. It depends on no compiler crate. A `dev-dependencies` edge only; it ships in nothing | [Contributing](../../CONTRIBUTING.md) |
| `marrow-test-programs` | Compiles and verifies shared source fixtures for lifecycle and runner tests. Compiler tests use the input helpers in `marrow-test-support`, avoiding a dependency cycle through their own compiler | [Contributing](../../CONTRIBUTING.md) |

The language server is its own executable. The `marrow` CLI has no `lsp`
subcommand.

## Production dependency direction

Every dependency points at a lower level. A crate names only crates beneath
it, so a change in a leaf rebuilds the leaf and its consumers and nothing else.

```text
marrow (CLI)        marrow-lsp
marrow-runner       marrow-project-fs
marrow-vm                                marrow-compile
marrow-lifecycle                         marrow-project
marrow-kernel       marrow-verify        marrow-syntax      marrow-local-wire
marrow-store        marrow-image         marrow-fs-journal  marrow-codes  marrow-temporal
```

Four leaves have no workspace dependency at all: `marrow-codes`,
`marrow-temporal`, `marrow-image`, and `marrow-fs-journal`. The compiler
reaches `marrow-image` but never `marrow-verify`, `marrow-vm`, or
`marrow-store`: it can emit bytes and cannot mint a verified image or open a
store. The VM reaches `marrow-lifecycle`, `marrow-kernel`, and `marrow-verify`
but never `marrow-compile`: it cannot see source, and it re-exports from the
lifecycle only the preparation and fresh-test surface the CLI consumes, never
provision, attach, or import. The language server reaches `marrow-compile` and
`marrow-project-fs` and nothing below the image.

## Tracing a command

`marrow test` shows the whole stack in one invocation. The CLI asks
`marrow-project-fs` to capture the project, reading the root tree and every
declared dependency's tree against one set of limits; `marrow-project` turns the
captured bytes into a `ProjectInput` whose every file carries its origin.
`marrow-compile` checks every module and lowers a
test image, which `marrow-image` encodes and `marrow-verify` seals.
`marrow-lifecycle` prepares the sealed image once and selects each `test` block
from it; `marrow-vm` runs the body. A body that touches durable data runs against
a store the lifecycle mints in memory from the prepared image, through
`marrow-kernel` over the in-memory engine in `marrow-store`. The store is
dropped when the test returns.

`marrow run <export> --store <dir>` replaces the last step. `marrow-lifecycle`
acquires the native owner lock and admits the prepared image against the stored
binding before engine open. `marrow-runner` dispatches the export through the
returned attachment over the persistent redb engine.

In both, the CLI's `cmd_run` materializes positional or stdin arguments against
the verified export signature and renders the outcome under its own byte limits
([CLI](../tools/cli.md)). Delivery failure can follow a completed invocation; it
does not undo or retry it.

`marrow doctor --store <dir>` stops before any export runs. The CLI compiles
and hands the image to `marrow-runner audit`; `marrow-lifecycle` admits it as
the store's exact active binding under the lock and opens the native engine
with `NativeOpenAccess::ReadOnly`. The kernel's logical walk checks every cell,
and the lifecycle returns findings and an entry-content digest. Physical
integrity is not checked, and inspection leaves an inherited unclean-shutdown
obligation undischarged ([storage](storage.md#auditing-a-store)). This report
grants no recovery or admission permit.

`marrow backup` shares project capture and image staging with doctor through
`cmd_store` unless `--image` selects an explicit artifact, bypassing capture and
compilation. The runner's `store_transfer` command module reads bounded image
bytes and delegates their single verification and exact active-store admission
to lifecycle backup. `marrow restore` bypasses project capture and compilation;
the runner streams the file to lifecycle restore, which owns embedded-image
verification, construction and final admission. Transfer receipt delivery is
fallible and retains known results in a best-effort diagnostic on failure.

`marrow apply` bypasses project capture through `cmd_store`. The runner's
`store_apply` module loads and verifies the explicit OLD and NEW artifacts
sequentially, then calls lifecycle apply once. Lifecycle compares the retained
verified graphs, extends the accepted physical map and standing ceiling, audits
OLD read-only, and publishes through the existing metadata owner. Receipt
delivery is fallible and retains the known outcome in a best-effort diagnostic.

`marrow recover --store <dir>` shares companion dispatch with doctor through
`cmd_store`. An explicit `--image` bypasses project capture and compilation;
otherwise it compiles the working project. The runner calls lifecycle recovery
once and writes a fallible result retaining preservation moves even on failure.
The lifecycle performs physical and logical validation under one owner and
establishes fresh barriers before returning. It neither returns an application
attachment nor rewrites the selected head
([storage](storage.md#explicit-recovery)).

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

[`ImageDraft`](../../crates/marrow-image/src/draft.rs) owns function identity
allocation. The compiler's
[`FunctionRegistry`](../../crates/marrow-compile/src/lower/registry.rs) retains
the reserved identities; lowering fills their slots, and semantic analyses use
those same indices.

[`ScopedName`](../../crates/marrow-compile/src/source.rs) is the one key shape
for a declared name — a type, a store root, a Product's resource spelling —
pairing it with the captured tree that declares it, so two trees may each
declare `Book` and an origin is never recovered from a spelling.

Independent verification is a separate trust boundary: the verifier reconstructs
types and demand from image bytes without consulting compiler state. Diagnostic
code spellings live in `marrow-codes`. The language server projects compiler
snapshot facts and owns their protocol representation and document state; a
missing semantic editor fact belongs in `marrow-compile`.
