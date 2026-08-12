# Implementation guide

This guide maps the code that exists at the current revision. It is descriptive
and intentionally shorter than the source: public language behavior belongs in
the [language reference](../language/), not here.

The beta line began at lane B00 with a deliberate capability trough: the
prototype's entangled compiler, interpreter, catalog, and durable owners were
deleted. A storeless compiler, program image, independent verifier, stack VM,
and a narrow path kernel over the retained ordered-byte engine have since been
refounded, so a small typed program now travels source to a reproducible image,
through the verifier and VM, to durable data and back. Provision/open/import
lifecycle exists; activation, evolution, backup, restore, packages, and served
execution are still being refounded lane by lane. [Project
status](../status.md) states what is current and what returns through which
direction.

## Crates

| Crate | Current responsibility | Read next |
|---|---|---|
| `marrow` | CLI: `init`, `fmt`, `check`, `run` (compile, verify, then execute an export storeless or through the native attached runner), `test` (compile a verified test image and run each `test`, driving a durable test against its own fresh ephemeral attachment), `import`, `image`, `lsp`, and `client typescript` (the deterministic strict-TypeScript generator emitting the pinned Node supervision module beside the generated client), plus typed not-yet-supported responses for refounding command names. Run and test output preserve incomplete invocation and durable-state facts separately | — |
| `marrow-codes` | Typed diagnostic-code registry and generated code reference | — |
| `marrow-project` | Pure project-input owner: manifest schema, contained discovery, immutable captured input, the read-only `.marrow/ids` semantic ledger, and the sole admitted identity-mutation/canonical-serialization path from an exact validated capture to an affine publication plan | — |
| `marrow-project-fs` | Bounded physical project-input adapter and the `.marrow/ids` publication owner: opened-handle admission of the project root, manifest, source tree, and identity ledger under fixed byte/visited-entry/depth bounds, one iterative sorted source walk, the consumer-neutral root-relative overlay input, and the capture-failure presentation facade; plus the project-metadata write guard, the kind-1 publication row header binding both exact byte runs and every inode witness, the closed publication state map, and one driver serving both a fresh publication and recovery of a crashed one. Capture refuses while a publication marker is live, so every front door inherits `project.ids_publication_pending`. Feeds root-relative bytes to `marrow-project` and depends only on it, `marrow-codes`, and `marrow-fs-journal` | [Projects](../tools/projects.md) |
| `marrow-syntax` | Lexer, parser, AST, formatter, and source diagnostics; owns the checked-format policy (`check_format`/`FormatRefusal`) both the CLI and the analysis snapshot consume | [Syntax](syntax.md) |
| `marrow-compile` | Storeless subset checker, language scalar vocabulary, and lowering to a program-image draft: it emits the whole durable graph's operation sites and lowers named `place` bindings with a once-evaluated key tuple and structured present-entry analysis. One dependency-resilient driver serves both the production compile (first-non-empty-stage projection) and the editor analysis floor: a revisioned immutable `AnalysisSnapshot` holding the caller's `ProjectInput`, the complete per-file diagnostic set, and selective hover/definition/format queries returning typed present/absent/unavailable facts with FIDB01-bounded file identities and full spans. One private fact ledger admits every retained editor fact against the snapshot's count and byte ceilings at the push that produced it, seals once into an opaque terminal, and retains no parse tree; see [Bounded analysis facts](#bounded-analysis-facts) | — |
| `marrow-image` | Canonical program-image container, typed draft, encoder, and image-id digest; a durable graph node's derived semantic path and a durable operation site's closed whole-payload/field-leaf target; the durable access-demand model (operation class, atom, and the `DemandSetId` demand-set identity); the deployment-ceiling descriptor whose read/write coverage and 32-byte ceiling-id are both derived from a demand union, binding a ceiling to its verified image; and the host-neutral wire interface descriptor, its closed transfer graph, and the `InterfaceId` interface identity | — |
| `marrow-verify` | The only image decoder and the phased verifier producing the sealed `VerifiedImage`; it resolves each operation site's semantic path against its own reconstructed node set and re-derives the site rather than trusting a compiler-side summary, and reconstructs each export's access demand from the sealed sites its call closure reaches | — |
| `marrow-vm` | Stack VM over the sealed instruction tape, with source-mapped runtime faults and a distinct durable-execution outcome: an incomplete invocation retains its fault code/span and independently carries known-old, known-new, unknown, or a pending opaque recovery fact. The ephemeral-attachment executor derives the store schema and site table from a verified image and runs a durable test against a freshly minted attachment | — |
| `marrow-kernel` | Path kernel: durable operation algebra, authority triple, store profile, checked-generation in-transaction commit witness, opaque affine commit-recovery fact and exact before/after classification, runtime logical codecs, the opaque native semantic owner over the lower engine-and-lock capsule, and the production ephemeral-memory attachment minted from a verified image | [Storage](storage.md) |
| `marrow-fs-journal` | Sole descriptor-rooted filesystem publication owner: entry-name admission, admitted-directory custody (`DIRECTORY`/`NOFOLLOW`/`CLOEXEC` admission, `CREATE`/`EXCL` mode-0600 creation, destination-refusing link, exchange/no-replace rename with typed fail-closed refusals), the affine close-on-exec cooperative cache lock, and the bounded five-kind `MWPEND0` pending-journal frame with claim/append/replay/exact-prefix-truncate/terminal-unlink states and crash-debris classification. The durability claim is atomic publication plus process/OS-crash recovery inside the file-and-directory-`fsync` envelope; macOS sudden-power-loss durability is not established. Qualification is a compile-time platform gate — macOS, or Linux on `x86_64`/`aarch64` — with a fail-closed stub whose one constructor returns a typed unqualified-platform refusal on every other target. `marrow-lifecycle` (owner-held store-artifact admission) and `marrow-project-fs` (kind-1 `.marrow/ids` publication) are its wired consumers, so a store open and an identity publication are both refused on an unqualified platform; lineage and package-cache publication rows consume it next | — |
| `marrow-lifecycle` | Privileged native-store provision/open/import composition: persistent instance identity and owner diagnostics, head-based admission supplied to the opaque kernel owner, and consuming commit recovery. An open takes the physical owner first and then reads the store directory's own `envelope` and `head` from a descriptor retained over it (`marrow-fs-journal`), each within the exact byte ceiling its own recorded version selects, so no state of those artifacts can preempt the exclusion verdict a contender is owed. It never owns or detaches the physical lock or native engine | [Storage](storage.md) |
| `marrow-store` | Ordered-byte engine contract, memory and redb backends, engine conformance suite, and the only public native-engine capability: an opaque engine-and-advisory-lock owner with non-returning create-only provision, a two-phase existing-only open (acquire the lock naming no store instance and making no engine call, then bind the instance and open under the same lock), mandatory recovery audit, and irreversible process-lifetime quarantine after an indeterminate commit | [Storage](storage.md) |
| `marrow-local-wire` | Pure single owner of the local wire: length-prefixed framing with a maximum size, the protocol version byte, canonical JSON (its own value model, codec, and depth/string bounds), the closed handshake/request/response/fault/incomplete grammar, the closed durable-state sum, and the closed loss classification; depends only on `marrow-codes` | — |
| `marrow-runner` | Storeless, ephemeral-durable, and native-durable export runner (library plus stock binary): the supervised Unix-domain channel discipline (mode-0700 dir, listener-before-handshake, launch nonce, poll-based deadlines, exact monotonic call turns, explicit fail-closed teardown, bounded loop-accept-until-authenticated), transfer codec, verified export dispatch, lifecycle recovery after an indeterminate commit, and outcome-unknown plus typed cause whenever no exact valid post-dispatch reply is accepted | — |
| `marrow-lsp` | The language server dispatched as `marrow lsp`: a private closed JSON-RPC 2.0 envelope and bounded standard-library stdio transport (no server framework, async runtime, or channel crate), a reader/coordinator/analysis-worker/writer topology over bounded channels and move-only capacity credits, the LSP lifecycle and full-document sync ledger, and diagnostics/formatting/hover/definition projected from `marrow-compile`'s published `AnalysisSnapshot`. Reconstructs no semantics and opens no store; consumes the fact surface plus `marrow-project-fs` | [Language server](../tools/lsp.md) |

## Guides

- [Syntax](syntax.md)
- [Storage](storage.md)
- [Testing](testing.md)
- [Diagnostic voice](diagnostic-voice.md)

## Ownership rule

One typed owner defines each semantic fact. Downstream crates consume stable
typed projections rather than matching source spellings, diagnostic prose,
raw paths, or serialized messages. When a needed fact is missing, add it to the
upstream owner; do not reconstruct it in the CLI, LSP, or tests.

## Bounded analysis facts

One private ledger in `marrow-compile` owns every retained editor fact. A hover
fact, a dependency gap, and a document-symbol node are each admitted against the
snapshot's typed count and byte ceilings **at the push that produced it**, before
the retained collection grows. No producer holds a fact carrier of its own, so
the ceiling bounds what one body holds live, not only what a whole project does,
and a producer stops rendering fact displays the moment the ledger is limited —
including part-way through the body it is lowering. Crossing a ceiling discards
the whole payload, including the already-admitted prefix, and seals the ledger
into an opaque terminal that `analyze` projects through one translation to the
public `SnapshotFactCount` or `SnapshotFactBytes` limit. Count keeps precedence
over bytes. No partial fact set is published under any provenance, and the
ledger's internal saturated totals are never published: a published total would
be a fabricated count.

The observable bounds are unchanged:

```text
MAX_SNAPSHOT_FACT_COUNT       = 65_536
MAX_SNAPSHOT_FACT_BYTES       = 4 MiB
MAX_DOCUMENT_SYMBOLS_PER_FILE = 4096
MAX_SYMBOL_DEPTH              = 16
MAX_COMPLETION_CANDIDATES     = 512
MAX_COMPLETION_RENDER_BYTES   = 256 KiB
MAX_ACTIVE_CALL_RENDER_BYTES  = 64 KiB
```

So are the logical byte charges: a hover fact charges its display plus the file
spelling of an optional definition target; a document-symbol module charges its
owner file spelling once plus every retained symbol-name spelling; dependency
gaps carry only fixed-size references and spans, so the count bound charges them.
Broken-module status is not a public fact row and is bounded by the 4096-file
project admission limit. A smaller physical representation never widens the
accepted fact bytes.

Retained facts name their file by a snapshot-local index into the project's own
module order, carry a definition target inline, and carry spans in the coordinate
domain the project owner already admits. These are representations, not
identities: only the snapshot that minted one can resolve it, through the same
coordinate validator every query uses.

**Accounted retention term.** The accounted retained representation of one live
`AnalysisSnapshot` is at most **11,116,544 bytes**, against an exported term of
`MAX_ANALYSIS_SNAPSHOT_RETAINED_BYTES <= 12 MiB`. It is an arithmetic property of
the pinned ceilings and the retained representation, not a sampled measurement,
and the exact figure is asserted rather than only bounded. Two exhaustive
destructures keep it honest: one over the snapshot's fields, so a new retained
field fails to build, and one inside each retained fact type's byte charge, so a
new heap-owning field on a fact fails to build there.

Two things sit outside the accounting. The caller-shared `Arc<ProjectInput>`,
whose up-to-64 MiB of source is the caller's charge and is shared rather than
copied. And per-allocation allocator overhead: the term charges structure sizes
plus charged string bytes, while a snapshot at the count ceiling holds up to
65,536 separate boxed strings whose allocator rounding and metadata a consumer
sizing a heap must add on top.

No parse tree is retained. Completion and signature-help queries re-parse exactly
one already-admitted file's already-retained bytes; the tree is transient, enters
no collector, and contributes no diagnostic. Parseability is never inferred from
such a parse — the snapshot's own broken-file record answers that — and the parse
is bounded by drive admission, which refuses a file whose accounted parse charge
exceeds `MAX_QUERY_PARSE_TRANSIENT_BYTES` before any file is parsed. The charge is
the file's byte length times the per-source-byte rate `marrow-syntax` publishes for
the representation its parser builds, so the refusal is arithmetic over two known
numbers rather than a measurement.

That gate narrows what is admitted when the representation widens because the heap
ceiling is declared independently of any file length — two thirds of the owned-heap
ceiling — and `MAX_PARSED_FILE_BYTES` is derived from it as the longest file whose
charge fits. The direction matters: a ceiling defined as what some chosen length
costs cannot gate that length, since comparing a file's charge against it reduces to
comparing lengths for any rate, and widening the representation would raise both
sides equally.

**Accounted transient terms.** Two working-set terms sit beside the retention
term for a consumer sizing a heap. Both are accounted from pinned ceilings and
the representation, not measured on a fixture; measurements corroborate them and
live with the lane's evidence.

```text
MAX_QUERY_PARSE_TRANSIENT_BYTES    <= 426.7 MiB   (declared: 2/3 of H_owned)
MAX_PARSED_FILE_BYTES               = 802,889 B   (derived: what that ceiling buys)
MAX_ANALYSIS_FACT_TRANSIENT_BYTES  <=  25 MiB
```

`MAX_QUERY_PARSE_TRANSIENT_BYTES` is the working set of **one** query-local parse
of one maximum admitted file: the tree, the lexer's token slice, the block
measurement that sizes each statement list, and the syntax collector's bounded
rows, live together. It is **declared**, not derived from a length: two thirds of
a 640 MiB owned-heap ceiling, keeping a third in reserve. The accounted charge of
a maximum admitted file is **422,502,609 bytes** (403 MiB), which closes under it
by the distance the per-family cap sits above the derived maximum.

The invariant it rests on is a **declared cap of 512 bytes per source byte, over
every node family the parser builds** — not a single total. The derived maximum is
481, over roughly twenty families, so shrinking the widest one promotes the next;
a total stated on its own would let a widened field hide behind whichever family
happened not to be deciding it. The cap is asserted per family. A widened field
moves its family's row, because the row is derived from the representation's
width; a field that owns a heap buffer is invisible to that width, so every
priced family also names all of its fields in a typechecked pattern, and adding
one fails to build until it is written down and priced.

The derivation runs in three steps. A `Statement` is the widest node the parser
stores in a list, and the grammar spends at least two source bytes on one — its own
token and the boundary that separates it from the previous statement, a newline or
the `}` closing a nested block — so a statement charges one slot plus one content
byte per further byte; which length is worst
depends on which of the two is wider, and both regimes are taken. A content byte
buys at most one call argument, expression node, or annotation in its widest
placement plus that node's own allocations. A container slot is charged at the
standard library's minimum non-zero capacity — four elements, since a container
holding one element allocates four slots and one element is the least a family's
own spelling admits — rather than at the doubling factor alone. The token slice and
the collector are live beside the tree. Multiplying by the admitted length gives the
figure. The statement slot itself is charged once rather than doubled: a statement
list is allocated at the measured count of statement *starts* its block opens
directly — a significant token following the block's `{`, a newline, or the `}` of
a nested block — and is held as a `Box<[Statement]>`, which has no capacity field
for slack to live in. Counting starts rather than lines is what makes the count an
upper bound: a compound statement's body closes on a `}` mid-line, so `if a {} if
b {}` is two statements on one line. The pass that measures a region is the same
one that decides the parser builds it, so a list sized at nothing and grown by
doubling is not representable.

Three properties of this term matter to a consumer. It charges allocated
capacity, so a `maximum resident set size` sample is a floor and not a check —
amortized growth slack is paid for by an allocator and never becomes resident. It
bounds one live parse, not a session: repeated queries are not free. And it is a
**capacity** bound only — it says the parse fits in the heap it is allowed and says
nothing about how long the answer takes. On the recorded host the densest maximum
admitted shape answers a completion query in 54 ms worst of five after a warm
query, down from 70 ms at the wider admission ceiling that preceded the corrected
accounting and 112 ms before the representation was shrunk; that is an improvement
and not immediacy, and the worst-shape latency case stays open.

Queryability itself depends on more than the admission ceiling. A file is
queryable only if its project yields a snapshot, and a single file that crosses
the analysis fact ceiling or the diagnostic ceiling refuses `analyze` outright,
so no snapshot exists to query. The densest queryable tree therefore comes from a
file whose nodes charge neither ceiling: a module that failed to parse, or names
that never resolve.

The two per-file outline bounds are narrower than that. A file whose declaration
hierarchy crosses `MAX_DOCUMENT_SYMBOLS_PER_FILE` or `MAX_SYMBOL_DEPTH` keeps its
snapshot: its own `document_symbols` becomes `Unavailable(Bounded)`, and every
other query for that file and every query for every other file is unaffected. No
truncated outline is retained — nothing at all is retained for that file — but one
oversized file no longer closes a project to its editor.

`MAX_ANALYSIS_FACT_TRANSIENT_BYTES` covers the peak attributable to producing
facts. Its dominant part is accounted: the live fact payload the ledger holds
while retaining is at most **21,176,320 bytes**, an arithmetic property of
`MAX_SNAPSHOT_FACT_COUNT`, `MAX_SNAPSHOT_FACT_BYTES`, and the project admission
limits, because admission stops at the ceiling at the push that produced each
fact and every charged spelling is held in an exactly sized `Box<str>`. No
workload can make that payload larger; a denser one only reaches the ceiling
sooner. The remainder — one display rendered and freed as it is charged — is
measured, as the difference between a fact-avalanche workload and an
identical-shape fact-free control (19 MB measured, on a hover avalanche crossing
the count ceiling inside a single body).

Both are distinct from the analysis **build** transient: `drive` materializes
every module's tree at once because cross-module resolution needs them. That
working set is named, not closed, by the bounded-fact work.

## Semantic availability and the image-policy fence

One semantic pass in `marrow-compile` produces eight private proof artifacts, each
minted by exactly one phase at the point that phase completes and taken by name by
every phase that depends on it:

```text
CompleteTypeRegistry ─┬─> SignaturesComplete ──────────────────────────> (encode only)
                      │
                      ├─> AcceptedQueuedTemplateProofs ─┐
                      ├─> CompleteDeclaredFunctionBodies┤
                      ├─> CompleteDeclaredTestBodies    │
                      │                                 v
                      │                  CompleteLoweredFunctionSet
                      │                                 │
                      │                        AcyclicCallGraph
                      │                        │             │
                      │       AmbientTransactionClosure      │
                      └─> value cycles   transaction ownership    mixed tests
```

The artifact named for the function registry is `SignaturesComplete`, a zero-size
proof that every declared signature resolved. `CompleteFunctionRegistry` is its sole
minter and the sole owner of the resolved signature table, whose own type is
`FunctionRegistry`; `encode` consumes the proof, never the table, so a resolved
signature table nothing vouches for stays unrepresentable at encode.

The signature table itself is always built. A signature the compiler refused — a
parameter or return type it could not resolve — is a refused entry in the table's
declaration ledger rather than a withheld table, so the phases below it still run: a
call to the refused function reuses its declaration's cause, and every unrelated body
lowers and reports its own errors. The proof is withheld, which is what fences the
program off from `encode`.

`CompleteDeclaredTestBodies` is not a prerequisite of `CompleteLoweredFunctionSet`: a
duplicate test title is a declaration refusal, not a lowering refusal, so the indices
actually minted stay dense and a call graph keyed by index over them is exact. It
withholds the instance drain alone.

An ordinary source refusal withholds exactly the artifacts that depend on it and no
others, so every independent phase whose own prerequisites still exist runs and
reports. A refused function signature, for example, withholds `SignaturesComplete`
and refuses that one declaration's body, while every other body, constant
evaluation, and the value-cycle audit still run. No phase is eligible because the
diagnostic set happens to be empty; each takes its own typed prerequisite. An
unavailable artifact never produces a substitute: no image entry, index, export,
test slot, or dependent fact is fabricated from a missing prerequisite.

`CompleteLoweredFunctionSet` and `AcyclicCallGraph` make their claim over the
functions that took an image index. A generic instance whose index was reserved but
never minted is outside that claim, and the instance drain runs only with
`AcceptedQueuedTemplateProofs`, `CompleteDeclaredFunctionBodies`, and
`CompleteDeclaredTestBodies` together — the artifacts that carry the drain's real
precondition, which is that every declared body took the index reserved for it.

The pass ends at a fence taken in exact order. A compiler-coherence invariant has
already returned. A non-empty diagnostic terminal — rows, or a terminal that
reported its own diagnostic ceiling — is the outcome. An empty terminal with any
artifact unavailable is an invariant, because an unavailable artifact always
follows a refusal that reported. Only an empty terminal with all eight artifacts
available is a checked program:

```text
semantic invariant > semantic diagnostic state (complete or overflow)
```

The image is encoded strictly after that fence, in the production projection alone.
So an image count or byte ceiling — the export table, the constant pool, the
function table — is a verdict about the artifact `compile` produces, never a
statement about the meaning of the program: it is unrepresentable in the semantic
outcome, and the analysis path, which never encodes, cannot reach one. A project
that crosses an image ceiling yields an ordinary snapshot with no diagnostic and
every query answering, while `compile`, `run`, `test`, `check`, and `client` all
report the same bound in the same words.

## Identity mutation admission

Project capture privately retains one `CapturedLedger`: the parsed semantic
ledger, an absent/present distinction, and the exact bounded raw bytes when
present. `IdentityLedger` exposes parsing and lookup only. The sole public
mutation operation is structurally nonempty and remains on `ProjectInput`; its
private planner sorts and validates the complete request, computes the exact
canonical successor size from semantic rows, and checks the row and byte bounds
before invoking the candidate supplier. Exact candidate count and collisions
are checked before the first base-ledger clone. A private admitted state then
serializes once and constructs the non-cloneable `LedgerPublicationPlan`.

The plan binds the exact captured expected state to one canonical successor.
Its only external access consumes the plan and borrows both halves together;
raw bytes cannot construct or split a plan through the API. The `marrow run`
bridge consumes this plan directly and writes only its successor through the
existing synchronized temporary-file-and-rename publisher. The physical
publisher does not yet compare the expected half against a fresh filesystem
state, so this boundary establishes neither stale-publication refusal nor
replay prevention.

## Product declarations and root occurrences

A durable **Product** is a declaration — the resource a `store` root projects — and
a root is an **occurrence** of it. `marrow-image` holds the two as flat tables: a
declaration table keyed by the Product's ledger identity, each declaration carrying
its member graph as parent-ordinal rows, and a root-occurrence row per root that
references the one declaration it projects. Nothing is retained per (root x member).
The v0 wire format is unchanged — it still carries the full member graph per root —
so the encoder projects each occurrence from its one retained declaration, and the
durable contract id is projected from the same rows.

A declaration member's stored value shape is a reference into one acyclic
`CanonicalValueShapeDag` per program, the sole representation of a durable value in
compiler, image, and verifier alike. Structurally identical shapes are interned to one
node, a node holds references rather than nested shapes — so it can state neither a
tree nor a cycle — and each node carries the longest path from itself down to a
scalar, computed as it is minted. That per-node metric is what decides the 32-level
nesting bound, so a shape reached at two depths is admitted or refused independently
at each. The v0 wire spells a value as its full expansion, which for a shared shape is
exponential in its nesting; both wire forms are therefore written by one iterative
walk straight into a byte sink that stops at a ceiling, and no expanded tree is built
in the compiler, in the contract preimage, or in the DURABLE section.

The encoder's two temporary bridges stand in front of that walk: a full-draft
coherence preflight replays every producer-side bound and coherence result in its
legacy order, and a durable-body lower bound then counts the DURABLE section before
building it, so a body no image could carry is refused before any buffer, contract
preimage, or output is allocated. The producer mints a durable-contract identity only
from the value that fence returns.

The identity carries its own ceiling as well. The canonical identity payload spells a
value as its expansion, so the length of that payload is not bounded by the size of the
graph describing it. `contract_id` therefore streams the payload into the hash without
materializing it and refuses a graph whose payload passes
`MAX_FITTING_CONTRACT_PREIMAGE_BYTES` instead of allocating it. That bound is derived,
not chosen: the preimage and the DURABLE body are the same walk over the same rows,
differing only in that the preimage spells a ledger reference in 25 bytes where the body
writes it in 16, so the widest payload a fence-admitted body can produce is that ratio of
the whole-image ceiling. Every graph an image can carry is therefore inside it, and the
refusal bounds the work of asking for an identity for every other caller.

### The durable contract graph is opaque

The graph is reached only through borrowed views. `DurableContractView<'_>` is a
zero-allocation non-owning view over the flat tables and the value arena; a root, a member,
and a member's kind are borrowed views whose payload structs have private fields and no
constructor. A caller can match a member exhaustively and walk its children, and cannot
state a graph of its own — there is no public recursive member type, no public raw field or
variant, and no entry point that accepts an already-built recursive owner by value. The
six raw `Durable*Shape` tree types and the `DurableContractDescriptor` they were built
into are deleted; what remains of that family is the two flat, non-recursive index rows a
managed-index declaration is stated as.

Both walks over the graph — the semantic-node enumeration and the payload writer — descend
an explicit stack of member runs rather than the machine stack, so a maximum graph's
traversal, equality, and teardown cost frames that do not grow with its nesting.

### Construction requires an admitted plan

Every entry point that adds to a durable graph — declaring a Product, occurring a root,
binding or requesting a site, and reading a declaration's members — takes an
`AdmittedGraphInputPlan`. The plan is count-frozen and is minted only by an admission
owner from its own census, so it cannot be spelled by a caller: its counts are private, and
`admit` refuses any term past what a program image could carry. A storeless compile carries
the empty plan, under which the constructing entry points refuse naturally. The plan bounds
*intake*; it classifies nothing, and the declaration graph's own command validation remains
the one structural validator of a command vector.

### An over-deep durable member is refused at its own span

A member tree nested past `MAX_DURABLE_DEPTH` is refused as a `check.resource_limit`
diagnostic at the offending member's span, from `marrow check` and `marrow run` alike and
whether or not the project's durable identities have been minted. No durable structural
bound is a public resource kind: the image's own depth, member-count, and index-component
defenses remain, but they are producer-side construction contradictions from a coherent
compiler rather than outcomes a program can reach, and the independent verifier answers a
hostile image with its own typed bound rejection.

**Accounted durable-graph terms.** What holding a durable contract graph costs is
published per element by `marrow-image` — one member command, one member row, one Product
row, one occurrence row, one managed index, one value node, one value reference — and each
admission owner derives its own maximum from those charges rather than sampling a fixture.

```text
compiler-side, at the identity ledger's admitted anchors    =  42,850,312 B  (<= 64 MiB declared)
verifier-side, at the whole-image ceiling                   =  72,876,264 B  (<= 256 MiB declared)
durable value arena, at the type population                 = 8,212,611,072 B  (exported, unbounded here)
```

The first two are asserted in a `const` context, so a representation change that breached
either ceiling fails the build. The third is an exported term, not a bound this layer
asserts: the arena's populations are bounded by the type population rather than by the
identity ledger, and tightening them belongs to the durable-value owner. A negative control
holds the first figure honest — the superseded representation, which expanded a member tree
per root occurrence, does not close under the same ceiling.

A declaration's branch entry records are materialized once for the Product, at its
first executable occurrence. `marrow-verify` reconstructs the declaration and its
occurrences independently and rejects an image whose two occurrences of one Product
identity claim different member graphs or different entry records.

An operation site is named by binding a live root-occurrence selector to a live
canonical declaration-path selector and the one target that node admits. The binder
is the sole mint path: a site is `(occurrence, declaration path, target)`, so one
declaration path under two occurrences is two sites, and no ordinal a caller holds
can name a place. `marrow-image`'s bounded site demand plan checks vacant capacity
before it mints an id, deduplicates by that key, saturates its logical count one past
the cap, and records one policy receipt at the earliest crossing.

How much of a Product's graph each occurrence pre-seeds depends on how many
occurrences it has, decided from a census over the whole store-declaration set before
any site is emitted. A Product with one occurrence pre-seeds its whole member graph
in declaration pre-order. A Product with more pre-seeds only each occurrence's root
whole-payload and index sites; its member group and branch sites are minted on first
reference, as a field leaf always was, so a declaration costs its referenced graph
rather than its declared graph multiplied by the roots over it.

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
