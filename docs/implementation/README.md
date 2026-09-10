# Implementation guide

Marrow's workspace separates syntax, compilation, verification, execution,
storage, and tooling into crates. The
[language reference](../language/) states what a program means; this guide
states where that meaning is computed.

## Pipeline

`marrow-image::IMAGE_FORMAT_VERSION` names the admitted image generation; its
digest domain separates payload identities between generations. The independent
container verifier checks that generation before decoding sections.
`marrow-lifecycle::LogicalHead::decode` checks the stored active binding against
the same generation. The shared owner-held head reader supplies attachment,
import and audit before engine open
([compatibility](../compatibility.md#versioning)).

A program travels one way. `marrow-syntax` parses `.mw` source into an AST.
`marrow-compile` checks the AST and lowers it to a program-image draft, which
`marrow-image` encodes to canonical bytes. `marrow-verify` is the only decoder:
it rebuilds every executable claim from the bytes and seals a `VerifiedImage`.
`VerifiedImage::function` checks an image-relative `FunctionIndex` and returns
a borrowed `VerifiedFunction` carrying the owner of its body and durable demand.
`marrow-vm::run` accepts that selection without a separate image. Internal calls
select from the current frame's image; lifecycle attachments retain image/store
pairing for durable execution. The raw storeless Rust entry requires correctly
typed arguments from that image and an empty demand; it does not validate those
preconditions. `marrow-vm` executes the selected instruction tape. Durable reads and
writes leave the VM through `marrow-kernel`, which encodes keys and values and
drives a transaction against an engine in `marrow-store`.

`marrow-vm::render` owns canonical value text for VM string conversion and CLI
output. One recursive append traversal shares a private destination with an
explicit caller byte limit. Nested aggregate values append into that destination;
temporal scalar formatting retains bounded scratch text.
Each variable-size contribution is checked before append, including the complete
hex expansion before its digit loop. VM conversion passes 65,536 bytes and maps
the typed refusal to `run.text_limit` at the current instruction. The CLI retains
its bare-string limit and has no aggregate-text byte ceiling. Its JSON hex, key
and identity contributions pass the existing data limit; JSON's separate encoding
and final admission checks remain in `outcome`. These checks bound constructed
text length, not allocator capacity, recursion or total runtime memory.

Verifier jump resolution records destinations with a predecessor other than the
immediately preceding instruction. Type flow carries one working frame through
adjacent destinations without another predecessor. A fork propagates its target
first, then reuses the working frame for a distinct eligible fallthrough.
Coincident edges still both propagate and meet. Entry zero and queued boundaries
retain frames for exact stack comparison and in-place definite-initialization
meets. Carried interiors retain only reachability and execute again when an
upstream meet weakens local facts. Straight-line padding and eligible fork
fallthroughs add no retained local or stack payload; instruction slots still
scale with code length. True joins and repeated visits remain separate costs,
without a total memory or work budget.

After every function passes type flow, `verify/context.rs` extracts direct-call
occurrences into one flat target vector with per-function offsets. The iterative
cycle check covers every function and records callee-first completion order.
Effects collect direct atoms, sites and transaction markers in one instruction
pass. A global atom lookup is consumed through the image demand canonicalizer;
its discovery remap builds sparse u32 selections in function order. The selection
owner normalizes direct ordinals once. Each recorded call then unions its
callee's completed selection into its caller with reusable merge scratch.
Transaction and test-entry checks reuse the graph, and duplicate calls retain
their tape order. Site closures remain separate sets. The graph stays live
through presence and test-entry checks; these passes have no total memory bound.

After validation, `verify/seal.rs` moves the canonical pool and selections into
the verified image. `sealed/demand.rs` owns their association with functions;
exports/tests retain function ordinals rather than owned demand copies.
`marrow-image`'s `demand/selection.rs` owns normalized selections and borrowed
views over its canonical atom owner. Views check the largest ordinal in constant
time and iterate only selected atoms; count and emptiness are constant-time.
Canonical payloads and identities share the existing encoder. The VM derives
session coverage from the invoked verified function's view. Owned ceiling and
union outputs remain independent values.

Sparse row length payload is four bytes per transitive membership, plus row
metadata and capacity. A merge retains its destination alongside replacement
scratch; repeated prefix work, site replication, lookup/remap/direct records,
canonical keys and sort scratch remain costs. Pooling removes deep atom copies
across overlapping demands, not total verifier residency or work limits.

The presence pass uses the same transient destination flags to require an uninterrupted
key-load, producer and consumer sequence before establishing a guard fact.
The flags are discarded before the verified image is returned; they add no
encoded or public image field. `verify/presence.rs` reconstructs entry-erasing
calls from the existing effect closure and checks each strict field/group
operation, including required field reads, against its containing entry and
complete key-slot tuple. Optional
entry and group reads establish facts only on their present branch.
The verifier's `EntryFamilies` borrows a sorted projection of validated entry
paths and branch coordinates, built once for the presence phase. Calls use
binary search without reconstructing key columns; direct erases use the same
entry-family classifier. This transient lookup is dropped before publication.

Presence verification carries one owned working set through linear segments.
A fork propagates its target first, then carries its distinct fallthrough when
that destination has no other predecessor. Coincident edges both intersect
before their destination executes. Shared and non-fallthrough destinations
retain incoming sets for intersection; a shrinking merge requeues the boundary
and re-executes its carried interior. Functions without strict presence
operations skip the pass. Otherwise it retains one optional state slot per
instruction. Carried instructions add no retained incoming fact sets; working
sets and branch-target copies remain. The guarded-read retention test checks
two retained sets and zero retained facts across six key-count/padding cases;
both retained sets are empty. This fixture-specific result does not bound
transient allocation, true-join retention, repeated visits or total verifier
memory and work.

`marrow-compile/src/lower/presence.rs` owns scoped presence facts with stable
identities and typed live or invalidated state. Invalidated identities remain
until lexical exit so a checked required read cannot become optional after
losing its proof. `lower/durable.rs` emits `DurReadFieldPresent` for required
fields through a live fact; optional field reads keep `DurReadField`. Required
group leaves use `DurReadGroupPresent` followed by `FieldGet`.
The lowerer logs each emitted call once and retains call-log
intervals for protected uses. `compile/presence_calls.rs` settles entry-erasure
closures with the existing acyclic call order, reusing one word per function
for each stripe of 64 queried families. Its pending interval chains avoid
copying calls into facts or rescanning a call slice for every use.

The VM's strict field arm uses the existing kernel field read and faults
`run.corruption` if the required value is missing. The read checks the selected
value, not the integrity of the whole entry.

`lower/durable.rs::resolve_index_read` resolves bracketed and bare index reads
to one declared index and a borrowed operand slice. Value expressions in
`lower/exprs.rs`, `exists` in `lower/durable.rs`, and `for` in `lower/stmts.rs`
use that result through their existing lookup, presence and scan lowerers.

`marrow-image/src/instr.rs` owns instruction tags and operand widths. Strict
field set, strict group read and group replacement carry explicit key slots;
replacement consumes only the group record from the operand stack. Retired
encodings reject in the verifier and require recompilation.

The compiler opens no store, and the VM accepts only an image the verifier
sealed. `marrow-lifecycle` prepares a verified image once, deriving the store
projection every engine opens under, and pairs the image with the store it
admits it for: a persistent store provisioned, attached, or imported through
the lifecycle (its file operations go through `marrow-fs-journal`), or a fresh
in-memory store for a durable `test`, discarded when the test ends. The VM
executes a durable export or test only through that pairing, so a store runs
exactly the image the lifecycle admitted for it.

The runner binary's shared `load_image` reads at most `MAX_IMAGE_BYTES + 1`
bytes through `Read::take` before calling the verifier. Every runner command uses
that loader. The verifier owns oversize and image-format refusal; open/read errors
retain the `io.read` path. The read bound does not establish allocation capacity
or a time bound for a stream that stops supplying bytes.

The compiler retains parser syntax. Its private `types/aliases.rs` owner stores
each supported alias as a shared global terminal name and optionality. It
normalizes chains iteratively and refuses unsupported target shapes before
dependent fills. Type consumers resolve written parameters before aliases and
carry existing declaration refusals through scalar and value-type checks;
they do not allocate expanded alias trees.

The image draft reserves each function identity once. Accepted ordinary signatures,
included test declarations and generic instances retain their actual `FuncId`;
lowering moves each completed instruction allocation into that reserved slot.
Failed bodies leave explicit vacancies. Template proofs use the same reserve/fill
operations and restore their slots and fills on exit. The draft's existing coherence
check refuses any vacancy before measurement or encoding. This does not change the
image format or successful function order.

The compiler retains full source coordinates and optional body facts at those same
indices. After settlement, transaction validation borrows the draft's instructions
and checks that the coordinates cover exactly that sequence. Iterative SCC analysis
reports cycles over available bodies. One further callee-first sweep excludes missing
or cyclic bodies and every transitive caller. Transaction and presence summaries and
reports use that shared membership while retaining the full reserved index domain.
Independent complete components remain diagnosable after an unrelated body refusal;
that restricted order does not establish whole-program readiness. Ordinary generic
body refusals leave queued work intact; resource and invariant failures still stop it.
An ambient-transaction diagnostic still suppresses subsequent transaction-ownership
checks for that drive to avoid cascading reports.

The draft keeps a saturating charge of the bytes its retained bodies alone commit
the image to (one byte per instruction plus one span row per span), snapshotted and
restored with its transactions. After each settled body the compiler polls it; once
the charge exceeds the image byte ceiling, compilation, `check`, and editor analysis
stop lowering and report the `ImageBytes` resource limit without a snapshot. An invariant
discovered in executed work is reported ahead of that stop and of parse or
structural findings. A stop retains at most 105,865 instructions: the largest prefix
under the charge (40,329 one-byte instructions) and the body that crossed it (at most
65,536); the lowerer's in-flight buffer for a later body is unretained. This is a
retention bound, not a capacity claim.

One drive of the compiler serves three projections. `compile` and
`compile_with_tests` report the first non-empty stage's diagnostics and encode
the production or test-inclusive image. `analyze` reports the complete union of
every stage's diagnostics and publishes the retained editor facts as an
`AnalysisSnapshot`, never encoding. `check`, which `marrow check` calls, drives
once with tests included, reports that same complete union, and encodes the
test-inclusive image once for the verifier; it reads no editor fact, so the
snapshot's fact retention bound does not refuse it.

A tool sees a project through two layers. `marrow-project` is pure: manifest,
module discovery, and the `.marrow/ids` ledger, all over bytes a caller supplies.
`marrow-project-fs` reads those bytes from disk under fixed bounds and publishes
the ledger. Both the CLI and the language server enter through `marrow-project-fs`.

## Crates

| Crate | Owns | Read next |
|---|---|---|
| `marrow` | The CLI: `init`, `fmt`, `check`, `run`, `test`, `import`, `doctor`, `image`, and `client typescript` | [CLI](../tools/cli.md) |
| `marrow-codes` | The diagnostic-code registry and the generated [error-code reference](../error-codes.md) | [Diagnostic voice](diagnostic-voice.md) |
| `marrow-syntax` | Lexer, parser, AST, formatter, and the diagnostic types every crate renders | [Syntax](syntax.md) |
| `marrow-temporal` | The `date`, `instant`, and `duration` domain: calendar, range, canonical text, and arithmetic. Depends on nothing else in the workspace | [Types and values](../language/types-and-values.md) |
| `marrow-compile` | The checker, the scalar vocabulary, lowering to the image draft, and the `AnalysisSnapshot` the language server reads | [Diagnostic voice](diagnostic-voice.md) |
| `marrow-image` | The program-image container, the validating `ImageDraft`, the canonical encoder, and the `ImageId` digest. Holds no decoder | [Compiled programs](../future/compiled-programs.md) |
| `marrow-verify` | The only image decoder and the phased verifier that seals a `VerifiedImage`; rebuilds each export's durable access demand from the image alone | [Trust boundaries](../status.md#trust-boundaries) |
| `marrow-vm` | The stack VM over a sealed image: source-mapped runtime faults, execution bounds (`value.rs::collection_within_limits`, `Value::structural_bytes`, `run.rs::bounded_list`), and durable execution of an export or a source test through the attachment the lifecycle prepared | [Execution limits](../language/execution-limits.md) |
| `marrow-kernel` | The path over which every durable read and write passes: key and value codecs, the operation algebra, the transaction commit witness, commit recovery, and the read-only audit walk | [Storage](storage.md) |
| `marrow-store` | The ordered-byte engine contract, the in-memory and redb engines, and the conformance suite both must pass | [Storage](storage.md) |
| `marrow-lifecycle` | The verified image's store projection and its pairing with a native or in-memory store; provision, attach, import, and audit of a persistent store: store identity, envelope, active head, admission, recovery after an interrupted commit, and the audit's digest | [Operations](../operations/README.md) |
| `marrow-fs-journal` | Descriptor-rooted file publication: entry-name admission, the cooperative lock, and the pending-journal frame with replay and crash-debris classification | [Storage](storage.md) |
| `marrow-project` | Manifest schema, module discovery, file identities, and the `.marrow/ids` ledger, all over caller-supplied bytes | [Projects](../tools/projects.md) |
| `marrow-project-fs` | Bounded reads of the project root, manifest, source tree, and ledger, and the sole publisher of `.marrow/ids` | [Projects](../tools/projects.md) |
| `marrow-local-wire` | The framed protocol between a runner and its client: framing, limits, canonical JSON, and the closed request, response, fault, and incomplete grammar | [TypeScript client](../tools/typescript-client.md) |
| `marrow-runner` | The runner binary and library: the supervised Unix-domain channel, export dispatch over a verified image, the transfer codec, collection admission, and canonical Map construction in `transfer.rs::decode_collection`, classification of an outcome the client could not confirm, and the one-shot provision, import, and audit commands | [Interrupted commits](../operations/README.md#interrupted-commits) |
| `marrow-lsp` | The standalone `marrow-lsp` executable: JSON-RPC over stdio, document sync, and diagnostics, formatting, hover, definition, completion, signature help, and document symbols projected from the compiler's `AnalysisSnapshot` | [Language server](../tools/lsp.md) |

The language server is its own executable. The `marrow` CLI has no `lsp`
subcommand.

## Dependency direction

Every dependency points at a lower level. A crate names only crates beneath
it, so a change in a leaf rebuilds the leaf and its consumers and nothing else.

```text
marrow (CLI)        marrow-lsp
marrow-runner       marrow-project-fs
marrow-vm           marrow-local-wire    marrow-compile
marrow-lifecycle                         marrow-project
marrow-kernel       marrow-verify        marrow-syntax
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
`marrow-project-fs` to capture the project; `marrow-project` turns the captured
bytes into a `ProjectInput`. `marrow-compile` checks every module and lowers a
test image, which `marrow-image` encodes and `marrow-verify` seals.
`marrow-lifecycle` prepares the sealed image once and selects each `test` block
from it; `marrow-vm` runs the body. A body that touches a `^` place runs against
a store the lifecycle mints in memory from the prepared image, through
`marrow-kernel` over the in-memory engine in `marrow-store`. The store is
dropped when the test returns.

`marrow run <export> --store <dir>` replaces the last step. `marrow-lifecycle`
admits the prepared image against the store's active binding, `marrow-store`
takes the engine lock, and `marrow-runner` dispatches the export through the
returned attachment over the persistent redb engine.

The CLI's `cmd_run` materializes positional or stdin arguments against the
verified export signature at the storeless and persistent call boundaries.
Stdin supplies exactly one bare string parameter; the argument reader materializes
at most 65,537 input bytes and admits at most 65,536 UTF-8 bytes. Standard input
buffering may read ahead. The persistent path discovers the companion before
consuming input. `outcome` checks returned bare-string size before rendering and
returns rendering refusals to `cmd_run`, whose emitter writes and flushes records
and fails on rendering or sink errors. Delivery failure can follow a completed
invocation; it does not undo or retry it. Aggregate text materialization remains
outside this string boundary ([CLI](../tools/cli.md)).

`marrow doctor --store <dir>` stops before any export runs. The CLI compiles
and hands the image to `marrow-runner audit`; `marrow-lifecycle` admits it as
the store's exact active binding under the lock and opens the native engine
with `NativeOpenAccess::ReadOnly`. The kernel's logical walk checks every cell,
and the lifecycle returns findings and an entry-content digest. The owner lock
is released before the runner renders the report. Physical integrity is not
checked, and inspection leaves an inherited unclean-shutdown obligation
undischarged ([storage](storage.md#auditing-a-store)). This report grants no
recovery or admission permit.

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

Independent verification is a separate trust boundary: the verifier reconstructs
types and demand from image bytes without consulting compiler state. Diagnostic
code spellings live in `marrow-codes`. The language server projects compiler
snapshot facts and owns their protocol representation and document state; a
missing semantic editor fact belongs in `marrow-compile`.

The server's current analysis is pending, a ready snapshot, or a typed resource
stop for the current input revision. Requests use the same reauthorization and
outbound-credit path for both completed outcomes. The exclusive publication
plan owns stop notices and diagnostic retractions; a pending publication retains
only its revision and is discarded if that revision is no longer current.

## Artifact fence

The [resource fill pass](../../crates/marrow-compile/src/types/build.rs) publishes
group identities after resolving field types. Generic fields may have already
built the metadata directory, so a successful fill phase invalidates that cache
once if it published groups. The next metadata query rebuilds from the completed
owners; later queries reuse the directory. A phase without groups retains the
existing classification.

The compiler captures public aggregate parameter and bound durable value roots
during signature and binding resolution. After signature construction commits,
`TypeMetadataSession` walks their shared resolved value graph, preserving
metadata invariant failures and reporting a source refusal for nominal-bearing
boundaries. It follows actual fields, payloads and collection components;
phantom generic arguments are validated as metadata but are not value edges.
The language reference owns the supported
[nominal boundaries](../language/types-and-values.md#aliases-and-nominal-ints).

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
