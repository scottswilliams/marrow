# Lane 14A Online Evolution Foundation Research

Date: 2026-06-04

Scope: architecture research only. This memo does not implement Rust and does
not create a new ADR. It records the accepted direction for Marrow's long-term
online evolution foundation after the Lane 14A review.

## 1. Executive Recommendation

Marrow should use a staged foundation:

- **v0.1:** strict single-epoch, fail-closed activation. A binary supports
  exactly its accepted catalog epoch and schema digest; `evolve apply` consumes
  an exact preview witness and publishes only after data and catalog are in
  lockstep.
- **v1 foundation:** shape activation as a compiler-owned job even when v0.1
  executes it immediately. The job/receipt records execution evidence, not
  semantics.
- **Future OLTP/server:** support MVCC/catalog-versioned online activation.
  Reads pin a snapshot and catalog epoch. Writes run through checked write plans
  that either preserve every active/building fact or fail closed.
- **Compatibility windows:** admit old compiled programs only through explicit,
  bounded, compiler-generated adapters. Old writes are rejected unless the
  compiler proves they lower to the latest write plan and maintain every active
  derived structure.
- **Major reshapes:** use a rare shadow-decant protocol for key changes,
  resource reshapes, layout recompiles, or engine moves that cannot safely
  backfill in place.

Marrow should not stay a single-epoch system forever, because OLTP workloads
cannot fence all data and processes for large changes. It should also not copy
SQL migration systems. The foundation remains Marrow's central law: source,
catalog, typed facts, runtime, engine profile, and durable data are one checked
system. Job state, receipts, and tooling are evidence over that system, not a
second schema model.

## 2. Precedent Research With Citations

PostgreSQL's online DDL is useful as an operational warning. `CREATE INDEX
CONCURRENTLY` allows normal reads and writes while building an index, but it uses
multiple scans, waits on concurrent transactions, and can leave an invalid index
that still consumes maintenance work if the build fails. PostgreSQL also
supports delayed validation for constraints through `NOT VALID` / `VALIDATE`
patterns, separating new-write enforcement from old-data proof. Sources:
[CREATE INDEX](https://www.postgresql.org/docs/current/sql-createindex.html),
[ALTER TABLE](https://www.postgresql.org/docs/current/sql-altertable.html), and
[explicit locking](https://www.postgresql.org/docs/current/explicit-locking.html).

CockroachDB and Google's F1 are the strongest online schema-change precedents.
Both model schema changes as state machines rather than one opaque migration
step. The important lesson is staged visibility: non-public/write-only states
let new writes maintain new structures before old data is fully backfilled, and
read visibility is published only after validation. CockroachDB documents online
schema changes and its newer declarative schema changer as job-based schema
change execution through the SQL layer and background jobs. F1's paper describes online,
asynchronous schema changes with intermediate states such as delete-only and
write-only, plus a rule that bounds concurrently active schema versions. Sources:
[CockroachDB online schema changes](https://www.cockroachlabs.com/docs/stable/online-schema-changes),
[CockroachDB SQL layer](https://www.cockroachlabs.com/docs/v26.2/architecture/sql-layer),
and [Online, Asynchronous Schema Change in F1](https://research.google.com/pubs/archive/41376.pdf).

Cloud Spanner reinforces the same operational point: schema updates are
long-running operations that avoid downtime, but validation and backfill can take
time and old schema versions consume resources while they remain active. Source:
[Spanner schema updates](https://cloud.google.com/spanner/docs/schema-updates).

FoundationDB is the best storage-layer precedent. It exposes ordered keys,
MVCC-style snapshots, optimistic transactions, conflict ranges, and a layering
model where higher layers own records, indexes, and schema. Its documented
limits discourage long, huge transactions; online work must be chunked,
checkpointed, and retryable. The FoundationDB Record Layer adds metadata
versions, index states, and schema evolution above the KV substrate. Sources:
[FoundationDB developer guide](https://apple.github.io/foundationdb/developer-guide.html),
[FoundationDB known limitations](https://apple.github.io/foundationdb/known-limitations.html),
and [Record Layer schema evolution](https://foundationdb.github.io/fdb-record-layer/SchemaEvolution.html).

Vitess, gh-ost, and pt-online-schema-change are useful for the shadow-copy
family. They copy data in chunks, keep source and shadow structures synchronized,
and perform a narrow cutover. Marrow should borrow the bounded job and cutover
discipline, not their SQL/table-copy semantics. Sources:
[Vitess managed online schema changes](https://vitess.io/docs/24.0/user-guides/schema-changes/managed-online-schema-changes/),
[gh-ost](https://github.com/github/gh-ost), and
[pt-online-schema-change](https://docs.percona.com/percona-toolkit/pt-online-schema-change.html).

Schema compatibility systems support Marrow's identity and compatibility-window
direction. Confluent Schema Registry defines compatibility modes over schema
versions. Avro resolves reader/writer schemas through names, defaults, and
aliases. Protocol Buffers make field numbers durable and require deleted field
numbers/names to be reserved rather than reused. These systems prove the value
of stable identity and explicit compatibility, but Marrow can go further because
it also controls the checked runtime and durable write planner. Sources:
[Confluent schema evolution](https://docs.confluent.io/platform/current/schema-registry/fundamentals/schema-evolution.html),
[Avro schema resolution](https://avro.apache.org/docs/1.12.0/specification/#schema-resolution),
and [Protocol Buffers reserved fields](https://protobuf.dev/programming-guides/proto3/#reserved).

Datomic is a useful immutable/log-oriented precedent because schema is data,
idents can be renamed while preserving identity, and invariant-violating schema
changes fail until data is made valid. Source:
[Datomic schema change](https://docs.datomic.com/schema/schema-change.html).

Erlang/OTP hot code loading is the runtime precedent. Code versions can coexist
briefly, state conversion is explicit, and long-lived processes must move across
versions deliberately. The Marrow analogue is runtime generations plus explicit
catalog-epoch compatibility. Sources:
[Erlang code loading](https://www.erlang.org/doc/system/code_loading.html) and
[release handling](https://www.erlang.org/doc/system/release_handling.html).

## 3. Marrow-Specific Leverage

Normal databases see DDL, table/index metadata, and bytes. Marrow sees more:
source declarations, accepted catalog identity, presence proofs, typed saved
places, checked transforms, effect facts, write plans, generated indexes, engine
profile, layout epoch, and eventually typed API contracts. That gives Marrow
three advantages conventional systems do not have:

- It can reject old writes because their checked write plan cannot maintain a new
  derived fact.
- It can generate adapters from source/evolution facts rather than from ad hoc
  runtime shims.
- It can make activation progress a compiler/runtime fact instead of an
  operator-maintained migration script.

The advantage is valid only if Marrow refuses a second semantic model. Migration
scripts, raw store patching, source-diff identity, permanent dual writes, and
hidden compatibility branches would discard the leverage.

## 4. Candidate Designs And Tradeoffs

### Strict Epoch Cutover

Reads run under the old epoch until apply. Apply fences writes, transforms all
affected data, rebuilds derived structures, stamps metadata, and publishes the
new catalog epoch in one local activation. Old compiled programs fail closed.

This is the right v0.1 contract. It is simple, auditable, and aligned with the
local embedded target. It is not enough for large OLTP evolution because major
backfills and index builds cannot hold a global process/data fence for the whole
operation.

### MVCC/Catalog-Versioned Online Activation

Reads pin a snapshot and catalog epoch. Writes are mediated by compiled epoch
facts and runtime generation. Background jobs backfill chunks. A new index or
field can be hidden/write-only/building while writes maintain it; the tiny
publish step advances readable catalog state only after verification.

This is the recommended long-term foundation. It keeps compiler authority while
borrowing the staged-state lesson from CockroachDB/F1 and the chunked transaction
lesson from FoundationDB.

### Compiler-Generated Compatibility Adapters

During an explicit compatibility window, the server may present an old typed
view and lower old writes through checked adapters. Adapters are generated from
source/evolution facts, named, epoch-bounded, visible in tooling, and deleted
when the window closes. If the compiler cannot prove an old write preserves new
required fields, indexes, resource shapes, and constraints, that old write is
rejected.

This is necessary for rolling server deploys, but it is also the highest-risk
surface. Hidden adapters would become permanent migration code. The default
policy should be read-only or reject-old unless a write adapter is proven.

### Shadow Decant

For key changes, resource reshapes, layout recompiles, engine moves, or other
changes that cannot safely backfill in place, Marrow creates a new shadow
store/layout, copies in chunks, bridges a bounded set of writes, verifies
bijective identity/count/checksum facts, and atomically switches the public
binding. The old layout is purged after compatibility closes.

This is the right rare path for major reshapes. It must never pretend that raw
key shape changes preserve identity by default.

## 5. Recommended Online Activation Protocol

The future protocol is:

1. **Preview:** emit an exact witness: source digest, old/new catalog digests,
   layout/engine profile, affected stable IDs, counts, transform/default
   digests, approvals, adapter candidates, and planned job states.
2. **Start:** consume the exact witness and create durable activation job state.
   Install hidden or write-only facts if the change needs staged writes.
3. **Bridge:** route new writes through latest checked write plans and maintain
   any public plus building derived state. Old writes are rejected unless a
   checked adapter proves them safe.
4. **Backfill:** process bounded, idempotent chunks with checkpoints, input
   ranges, counts, checksums, and retry state.
5. **Verify:** prove base-to-derived and derived-to-base integrity, required
   field coverage, transform determinism, uniqueness, and absence of drift.
6. **Publish:** atomically advance readable catalog epoch/runtime generation and
   stamp the activation receipt.
7. **Close:** drain or reject old generations, delete adapters, purge retired
   state, and mark the compatibility window closed.

v0.1 may collapse start/backfill/verify/publish into one local apply transaction,
but the type and receipt shape should not block the future protocol.

## 6. Required Invariants

- Source, accepted catalog, checked facts, runtime, engine profile, and durable
  data are the only semantic model.
- Job state and receipts are evidence, not executable migration history.
- Backfill chunks are deterministic, idempotent, bounded, and restartable.
- Transforms, defaults, and adapters are pure, effect-free, digest-stamped, and
  total over their declared old-data domain.
- Indexes are not readable by production code until verified.
- Old writes fail closed unless lowered by a checked adapter to latest-format
  writes.
- Compatibility windows are finite and auditable. The normal window supports at
  most one old epoch.
- Destructive changes require exact-scope approval and cannot publish while an
  old generation still depends on the retired fact.
- A key-shape change is not identity-preserving unless a source-visible decant
  protocol proves the identity mapping.

## 7. Compatibility-Window Policy

v0.1 has no compatibility window beyond exact epoch/schema equality.

Future server mode may support a window from one old epoch to one new epoch. The
window records admitted client epochs, read adapters, write adapters, rejected
operations, start/publish/close epochs, active runtime generations, and deletion
conditions. Old clients default to read-only or rejected. Old writes require a
compiler proof that the adapter preserves every active and building invariant.

Closing the window is a first-class action: no active old sessions, all jobs
verified, all adapters deleted or marked expired, and all old write admission
removed.

## 8. Data Layout And Transaction Implications

Commit metadata needs enough room for future activation: catalog epoch, layout
epoch, engine profile digest, source/catalog digest, runtime generation, changed
roots/indexes, optional activation job ID, and optional adapter set. Layout
epochs may coexist only under job ownership; ordinary code should still see one
logical schema at a time.

Large operations must run in chunks that fit the engine's transaction limits.
Chunk state must name the catalog/layout epoch it read, the key range or cursor,
input high-water mark, output counts, checksum, and retry status.

## 9. Tooling/API Implications

Developer tooling should render outcomes in a small vocabulary:

- `safe`
- `needs apply`
- `needs job`
- `blocked`
- `needs approval`

Operator tooling can show the internal state: hidden, write-only, building,
verified, published, closing, closed, failed, or repair-required.

Future API clients must present compiled source/catalog identity and supported
epoch range. Server admission pins a runtime generation or rejects the request
with a typed stale-client error. Raw saved paths, raw payload bytes, and route
spelling are never the compatibility contract.

## 10. What To Do In v0.1 Now

- Keep strict exact-epoch activation.
- Shape apply as an activation job/receipt even if it executes immediately.
- Stamp commit metadata at the physical durable commit boundary.
- Preserve source/evolution digests and affected stable IDs in activation
  evidence.
- Keep transforms pure, deterministic, per-record, and idempotent.
- Keep index rebuild visibility all-or-nothing.
- Do not freeze APIs that assume Marrow can only ever have one catalog/layout
  epoch in the store.

## 11. What To Defer

- Multi-epoch storage.
- Server admission and runtime-generation draining.
- Old-client adapters.
- Shadow decant.
- Online index visibility.
- Distributed leases or multiple writers.
- General migration scripts or host-code migration hooks.

## 12. Risks And Red-Team Objections

The main risk is hidden compatibility glue. Any adapter not named, bounded, and
deleted becomes a permanent migration. The next risk is job state becoming a
second schema ledger. Receipts must never decide semantics. The third risk is
old writes: rejecting too often hurts rollout ergonomics, but accepting one
unsafe write corrupts the compiler/data law. The fourth risk is unbounded scans
presented as a production contract. Every backfill must be chunked and
checkpointed.

## 13. Open Questions For Scott

Scott accepted the core recommendations on 2026-06-04. Remaining product
questions are implementation policy rather than architectural direction:

- Should old clients be read-only by default in every compatibility window?
- Should the normal future server window be exactly one old epoch?
- Should required-field `default` mean a temporary activation fill only, or also
  a durable read default?
- Should re-key ever be identity-preserving, or always a new store plus explicit
  transform/decant?

## 14. Proposed Updates To Lane 14, Lane 15, Lane 16, And Lane 19

Lane 14 should treat activation as job-shaped evidence from the start, while
keeping v0.1 exact and fail-closed. It must reject job state as semantic
authority and prove no partial catalog/data visibility.

Lane 15 should make commit metadata describe the whole durable commit boundary
and leave room for runtime generation, activation job ID, and adapter evidence.

Lane 16 should add activation status, adapter/window rendering, and bounded
job-progress facts to the shared tooling-facts backlog while preserving raw
surfaces as debug/admin only.

Lane 19 should add a product decision packet for compatibility-window policy:
one-old-epoch default, old-write admission, required-default meaning, and re-key
identity semantics.
