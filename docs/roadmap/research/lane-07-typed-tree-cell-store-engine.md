# Lane 7 Typed Tree-Cell Store And Engine Profile Audit

Date: 2026-06-04

Scope: Lane 7 storage architecture research for Marrow v0.1. This is an audit
artifact only; it does not create an ADR or edit production code.

Inspected state:

- `/Users/scottwilliams/Dev/marrow` was clean, but its primary checkout was on
  `research/lane-09-source-native-evolution-audit`, not `main`, at `854cd15`
  when first inspected.
- `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit` is the report
  worktree, on `research-lane-07-tree-cell-store-audit`. It was created from
  `origin/main` at `854cd15`, then fast-forwarded before validation to current
  `origin/main` at `92941e4` (`docs(roadmap): add lane 5 resource store audit`).
  That newer commit only added a separate Lane 5 research file and did not change
  the Lane 7 store files audited here.
- `/Users/scottwilliams/Dev/marrow-decisions` was on `main` at `cae3e77` with
  unstaged edits in
  `/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md`
  and
  `/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md`.
  Those edits are included in this audit.

## 1. Local Vision Summary With File/Line References

The local vision is one language/database machine, not a raw storage API with a
language layer beside it. The top-level engineering law says durable data is
subject to the compiler regardless of engine, raw byte validity is insufficient,
and source spelling, public path text, physical store keys, and stable schema
identity must not collapse into one concept
(`/Users/scottwilliams/Dev/AGENTS.md:138`,
`/Users/scottwilliams/Dev/AGENTS.md:143`). The same file requires lane-local
cleanup of prototype paths, no green-test compatibility shims, and code-shape
review of broad dispatch and unbounded materialization
(`/Users/scottwilliams/Dev/AGENTS.md:23`,
`/Users/scottwilliams/Dev/AGENTS.md:28`,
`/Users/scottwilliams/Dev/AGENTS.md:48`,
`/Users/scottwilliams/Dev/AGENTS.md:125`).

The canonical backend contract says the v0.1 store is a typed tree-cell store
over a private ordered-byte engine. Production callers use tree-cell operations
and do not receive raw engine keys, saved-path encoders, backend traversal
traits, or archive replay APIs
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/backend-contract.md:3`).
It explicitly limits the public store surface to `cell`, `key`, `tree`,
`value`, `StoreError`, and `Decimal`
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/backend-contract.md:10`),
while the private substrate owns byte-lexicographic reads, writes, prefix
delete, bounded scans, cursor scans, and savepoint transactions
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/backend-contract.md:22`).
Physical keys derive from catalog IDs, typed key values, and the reserved
placement prefix, never from source names or order
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/backend-contract.md:39`).

Lane 7's roadmap now describes the same boundary: `TreeStore` is the production
model, `backend`, `mem`, `redb`, and `traversal` are implementation modules, and
`path`, `archive`, and `debug_admin` are not public production modules
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-07-tree-cell-store-engine.md:19`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-07-tree-cell-store-engine.md:30`).
The lane also records the consumer migration contract: there is no replacement
for public raw saved-path parsing, raw physical key encoding, root/child/sibling
traversal, raw prefix scans, raw max-int scans, or raw archive replay
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-07-tree-cell-store-engine.md:83`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-07-tree-cell-store-engine.md:122`).

The language docs reinforce the architecture. Durable identity belongs to an
invisible catalog, and renaming source fields does not move data
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:179`).
Indexes are store-owned generated lookup trees, ordinary code reads them, and
ordinary code does not write them
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:222`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:283`).
Lookup and traversal are explicitly written in source and stream lazily; Marrow
has no separate storage query language
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:286`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:320`).
Typed backup/restore is deferred until a tree-cell backup manifest exists, and
generated index data is not restored by treating raw saved paths as production
backup
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:541`).
Raw untyped writes to managed roots are rejected
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/resources-and-storage.md:601`).

The cost model and the unstaged decision edits make this more explicit. The
language docs say the access path is the source and there is no query optimizer
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/cost-model.md:3`).
The minimal-plan guarantee says the planner may elide provably redundant
operations, but never chooses between semantically distinct plans by runtime
statistics
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/language/cost-model.md:51`).
The dirty foundations ADR adds that no layer below the language can perform the
same program in fewer operations
(`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:56`),
and its consequences now foreclose a cost-based optimizer below the language
(`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:68`).
The dirty transactions ADR mirrors that for write planning
(`/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md:28`).

Important local tension: roadmap files do not fully corroborate the prompt's
"lanes 5-10 complete" claim. The execution plan lists Lanes 5-9 as completed
foundations, but says Lane 10 is the next active quality intervention
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/prototype-to-v1-execution-plan.md:173`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/prototype-to-v1-execution-plan.md:193`).
The Lane 10 doc itself says tracked edits wait for relevant fact, store,
runtime, and evolution contracts, and that lane completion requires rebuilt or
deleted/demoted tools plus typed backup/restore/protocol code
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:14`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:31`).

## 2. Implementation Summary With Crate/Module References

The store crate has the right public/private split in code. Its crate root makes
`backend`, `mem`, `redb`, and `traversal` private modules while exposing
`cell`, `decimal`, `key`, `tree`, and `value`
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/lib.rs:9`).
The store source tree contains no `path.rs`, `archive.rs`, or `debug_admin.rs`
(`rg --files crates/marrow-store/src` in the report worktree).

The raw substrate still exists, but only as private engine law. `Backend` is
`pub(crate)` and speaks raw `&[u8]` keys and `Vec<u8>` values
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/backend.rs:60`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/backend.rs:66`).
`MemStore` is `pub(crate)` over a `BTreeMap`
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/mem.rs:9`).
`RedbStore` is `pub(crate)` and opens either a writable `redb::Database` or a
`ReadOnlyDatabase`
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/redb.rs:42`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/redb.rs:52`).
Read-only handles reject write capability before beginning write transactions
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/redb.rs:66`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/redb.rs:226`).

The physical key profile is private. `CatalogId` and `DataPathSegment` are
public typed address pieces, but `CellKey` and the key byte constructors are
`pub(crate)`
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/cell.rs:26`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/cell.rs:86`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/cell.rs:90`).
Typed key values live in `SavedKey`; order-preserving key codecs are private,
while the identity payload codec is public because unique indexes and identity
leaves use it as typed payload
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/key.rs:5`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/key.rs:88`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/key.rs:100`).
Value codecs are public as canonical scalar payload bytes, with type supplied
by checked facts rather than stored type tags
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/value.rs:1`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/value.rs:156`).

`TreeStore` is the production facade. It owns a private `Box<dyn Backend>` and
exposes `memory`, `open`, `open_read_only`, transaction methods, metadata, node,
leaf, sequence, nested-data, record-child, and index operations
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:143`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:148`,
`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:173`).
Exact index tuple scans validate opaque cursors against the exact tuple prefix
and filter by the private key range
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:1100`).
Commit metadata now includes a source digest in code
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:60`),
although the backend contract's metadata table omits that field and should be
updated before freeze.

Runtime consumers have moved to typed store imports. `marrow-run` imports
`CatalogId`, `SavedKey`, `DataPathSegment`, and `TreeStore`, not removed raw
store modules
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-run/src/store.rs:1`).
The runtime architecture tests include absence checks for `marrow_store::backend`,
`marrow_store::path`, `PathSegment`, and `encode_path`
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-run/tests/architecture.rs:421`),
plus checks against runtime-local saved-path classification
(`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-run/tests/architecture.rs:567`).

Implementation shape is improved but not finished. A scan found no `unsafe` and
no `#[allow(...)]` in `crates/marrow-store/src` or `crates/marrow-store/tests`.
However, `tree.rs` is 1,924 lines and mixes facade, metadata codecs, data cells,
index scans, reference/enum codecs, corruption fixtures, and some private
test-only backend manipulation. That is not a reason to reverse the architecture,
but it is a Rust-shape risk before v0.1 API freeze.

## 3. External Precedents And Counter-Precedents

- SQLite: the official file format has separate table and index B-trees, with
  table leaves storing data and index B-trees using arbitrary keys. That supports
  Marrow's choice to keep primary cells and derived index cells separate, rather
  than treating one raw path stream as the whole database. It is a counter-
  precedent to exposing physical bytes as the user model: SQLite's file format is
  durable, but SQL and schema remain the product contract. Source:
  https://www.sqlite.org/fileformat.html

- SQLite backup: the online backup API can make a bitwise snapshot of a live
  SQLite database. That is valuable for one-engine administration, but it is not
  the same as a portable logical contract. Marrow's rejection of raw archive
  replay as production backup is consistent with this split. Source:
  https://www.sqlite.org/backup.html

- FoundationDB: FoundationDB exposes a lexicographically ordered byte-key model,
  and its tuple layer provides robust order-preserving encodings for mixed data
  types. This is the closest positive precedent for Marrow's typed key codecs over
  a private ordered-byte substrate. Sources:
  https://apple.github.io/foundationdb/data-modeling.html and
  https://apple.github.io/foundationdb/api-python.html

- FoundationDB Record Layer: a mature high-level layer on ordered FoundationDB
  keys supports structured records, fields, schema evolution, primary and
  secondary indexes, and declarative queries. This supports building typed data
  models above an ordered key/value substrate, but it is also a warning: once the
  product chooses declarative queries, planning becomes a major subsystem. Marrow
  intentionally chooses source-written access paths instead. Source:
  https://foundationdb.github.io/fdb-record-layer/

- redb: redb describes itself as an ACID embedded key/value store using
  copy-on-write B-trees with MVCC for concurrent readers and a writer, crash
  safety, savepoints, and rollbacks. That matches Marrow's native v0.1 needs well:
  embedded local state, ordered bytes, snapshots, one-writer behavior, and no
  external server. Source: https://docs.rs/redb/latest/redb/

- LMDB: LMDB is the older and more mature embedded ordered-map precedent:
  lexicographically sorted keys, one concurrent writer, cheap read transactions,
  zero-copy iteration, and multiple readers. It validates the one-writer/many-
  reader storage shape. Its C and memory-mapped operational profile are not
  clearly better for Marrow v0.1 than redb's Rust-native integration. Sources:
  https://www.lmdb.tech/doc/group__readers.html and
  https://lmdb.readthedocs.io/en/latest/

- RocksDB and LSM engines: RocksDB's compaction docs emphasize the tradeoffs
  among write, read, and space amplification. LSM engines are credible future
  engines for write-heavy workloads, but they bring compaction tuning, iterator
  and snapshot cost, and operational complexity that Marrow does not need to put
  in the v0.1 product contract. The private backend substrate preserves this as
  an engine-replacement option. Source:
  https://github.com/facebook/rocksdb/wiki/Compaction

- Postgres: Postgres stores tables and indexes as fixed-size page arrays, and
  index-only scans work only because heap visibility is tracked separately in a
  visibility map. This is a counter-precedent to "index key is the record":
  mature systems make data/index/visibility boundaries explicit. Marrow can avoid
  Postgres heap/MVCC/vacuum complexity in v0.1 because redb supplies snapshots and
  Marrow has one writer, but Marrow should keep its data cells, index cells, and
  commit metadata distinct. Sources:
  https://www.postgresql.org/docs/current/storage-page-layout.html,
  https://www.postgresql.org/docs/current/indexes-index-only-scans.html, and
  https://www.postgresql.org/docs/16/storage-vm.html

- Postgres logical dump: `pg_dump` is a portable logical/archive mechanism rather
  than a raw physical byte contract. That strongly supports a typed Marrow backup
  manifest over raw engine file or raw archive replay. Source:
  https://www.postgresql.org/docs/16/app-pgdump.html

- Document/object storage: MongoDB's data modeling guidance frames embedding
  versus references as an access-pattern decision, with high-cardinality child
  data favoring references and indexes adding write cost. Marrow's tree cells are
  a middle path: record bodies can be adjacent, keyed child layers and indexes are
  explicit ranges, and references carry catalog-backed identity rather than source
  names. Source:
  https://www.mongodb.com/docs/manual/data-modeling/best-practices/#link-related-data

## 4. Alternatives Considered

1. Keep or restore public raw saved-path/backend/archive APIs.
   This should be rejected. It creates a second semantic owner, makes physical
   key compatibility look product-supported, and contradicts the accepted
   source/catalog/compiler/runtime/engine layering. There is no evidence in the
   inspected code that production callers still need those public surfaces.

2. Use SQLite as the internal store instead of tree cells over redb.
   SQLite is the strongest off-the-shelf embedded alternative because it has
   mature recovery, backup, tooling, and indexes. It is still the wrong product
   contract for Marrow v0.1: Marrow would either hide SQL behind a typed layer,
   gaining little over an ordered engine while accepting SQL schema/planner
   impedance, or expose SQL-like storage concepts and create a second language.
   SQLite could be a future private engine only if it stores the same tree-cell
   contract.

3. Store each resource as a document/blob.
   This is simpler initially and aligns with document-store ergonomics, but it is
   weak for single-field writes, exact index maintenance, high-cardinality keyed
   child layers, sparse absence, and typed traversal. It also tempts whole-record
   materialization, directly against Marrow's lazy traversal and no-hidden-scan
   laws.

4. Adopt a Postgres-like heap plus secondary-index architecture.
   This is the long-term high-scale relational shape, but it imports heap tuple
   visibility, vacuum, HOT-like update rules, page layout policy, and optimizer
   expectations. For local v0.1, tree cells over engine snapshots are simpler and
   more congruent with typed resource trees.

5. Choose RocksDB/LSM as the native engine now.
   An LSM engine may be better for high write throughput or large datasets, but it
   adds compaction and tuning before Marrow has workload evidence. The right move
   is keeping the ordered-byte substrate private so an LSM backend can be added
   behind the same typed contract later.

6. Freeze the proposed physical key ADR exactly as written.
   Not yet. The proposed ADR and current backend contract differ in key namespace
   details: the ADR proposes namespace bytes such as data/index/sequence/catalog
   (`/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/04-physical-key-and-value-encoding.md:28`),
   while the current backend contract uses placement/profile/family tags
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/backend-contract.md:44`).
   That is acceptable while the ADR remains Proposed, but it must be reconciled
   before any layout epoch or backup manifest is frozen.

7. Keep the current public `Vec<u8>` payload contract as-is.
   This is acceptable only if docs remain honest that these are canonical typed
   payload bytes, not backend records. A stronger API would introduce typed
   wrappers such as leaf/index/sequence payload bytes to make "not raw backend"
   structural rather than prose-only.

## 5. Verdict: Refine

Verdict: refine, not reverse.

The core foundation is right for Marrow v0.1. Typed tree cells over a private
ordered-byte engine fit the language's durable typed-tree model, source-written
access paths, store-owned indexes, catalog identity, exact index scans, and
engine replacement goal. Stable catalog IDs in physical keys are the decisive
choice: they prevent source names and declaration order from becoming durable
identity. Private redb gives the correct local engine profile: ordered bytes,
ACID commits, many read snapshots, one writer, read-only opens, and format
metadata.

The architecture is not "perfect" in the sense of ready-to-freeze every public
byte and facade shape. It is v0.1-acceptable if the refinement items below are
handled before the tree-cell API, backup manifest, and layout epoch become
compatibility promises. None of the findings justify going back to raw saved
paths or replacing the tree-cell abstraction with SQL, documents, or LSM as the
product model.

## 6. Long-Term Risks

1. Stale canonical docs can resurrect duplicate semantics.
   The store docs are clean, but an accepted backup/repair ADR still says raw
   inspection reads raw saved bytes and runs even when source does not check
   (`/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/05-backup-restore-and-repair.md:38`).
   Lane 10 docs still describe raw/path-addressed data and explain surfaces as
   active suspects rather than completed replacements
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:22`).
   Lane 11 has stale references to deleted `archive.rs`
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/roadmap/lanes/lane-11-rust-hardening.md:153`).

2. Public canonical payload bytes are easy to misread as raw values.
   `TreeStore::read_leaf`, sequence reads, data reads, index entries, and index
   values expose `Vec<u8>`
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:229`,
   `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:257`,
   `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:290`,
   `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:75`).
   The docs correctly call these canonical store value bytes, but the type system
   does not distinguish leaf payloads from index payloads, identity payloads, or
   arbitrary bytes.

3. Public child-key helpers can become hidden materialization.
   `data_child_keys`, `record_child_keys`, and `index_child_keys` return `Vec` and
   internally page until complete
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:327`,
   `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:401`,
   `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:539`).
   That is typed, not raw, but it weakens the language promise that iteration
   streams lazily and hidden traversal is rejected. `max_int_*` helpers are also
   typed scans and need explicit planner/runtime justification
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:392`,
   `/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:467`).

4. The store Rust shape is too broad for a frozen foundation.
   `tree.rs` at 1,924 lines is a large module containing facade methods,
   cell operations, metadata codecs, reference/enum codecs, index cursors,
   corruption handling, and tests. This is not slop severe enough to reverse the
   design, but it is below the lane's stated "senior production Rust" target.

5. Commit metadata docs trail implementation.
   The code records `source_digest` in `CommitMetadata`
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/crates/marrow-store/src/tree.rs:64`),
   while the backend contract metadata table lists commit id, catalog epoch,
   layout epoch, profile digest, and catalog ID lists but not the source digest
   (`/Users/scottwilliams/Dev/marrow-lane-07-tree-cell-store-audit/docs/backend-contract.md:103`).
   This is the kind of small doc drift that later becomes a compatibility dispute.

6. Proposed physical encoding and current implementation are not the same
   contract yet.
   The proposed ADR's namespace grammar and the current backend contract's family
   grammar differ. That is acceptable while proposed, but v0.1 cannot freeze both.

7. redb is a sound default but not a universal performance answer.
   redb/LMDB-style copy-on-write B-trees are excellent for local read-heavy and
   moderate write workloads. A future write-heavy profile may want an LSM engine,
   but that should be an engine-profile addition behind the same tree-cell
   contract, not a reason to expose raw backend APIs now.

8. Backup is the highest remaining semantic edge.
   The local docs say typed backup/restore is deferred; Lane 10 owns the manifest.
   Until a typed manifest exists, "no raw archive production API" is a deletion
   claim, not a complete backup story.

## 7. Concrete Follow-Up Recommendations Ordered By Foundation Risk

1. Reconcile storage decisions and docs before any v0.1 freeze.
   Update or supersede the accepted backup/repair ADR language that still blesses
   raw inspection; align Lane 10 status with reality; remove stale Lane 11
   `archive.rs` references; add `source_digest` to backend contract commit
   metadata; reconcile the proposed physical-key ADR with the actual key profile.

2. Make public payload bytes structurally typed.
   Introduce small newtypes or equivalent wrappers for canonical leaf payload,
   sequence payload, index payload, and identity payload, or explicitly decide that
   `Vec<u8>` is the stable payload contract and update every doc to say so. The
   preferred direction is wrappers: they preserve engine privacy in the type
   system and make misuse harder.

3. Replace materializing child-key APIs with bounded typed pages or cursors.
   Keep exact index tuple scans as the model. Add paged/cursor forms for record,
   data, and index children, then either remove the all-keys helpers or make them
   crate-private test conveniences. This is the highest code-level risk because it
   can silently undercut the no-hidden-scan language law.

4. Split `tree.rs` by invariant before declaring the store API frozen.
   Suggested modules: facade, metadata, data_cells, index_cells, child_scans,
   reference_values, enum_values, and commit_codec. This should be mechanical and
   semantics-preserving, with no compatibility bridge.

5. Keep raw backend/redb/mem APIs private and add absence tests at the store
   crate boundary.
   Runtime absence tests exist, but store crate tests should also prove
   production callers cannot import `backend`, `mem`, `redb`, `path`, `archive`,
   or `debug_admin`.

6. Treat typed backup manifest as a blocking Lane 10 foundation, not a nice-to-
   have tool.
   The store foundation is correct without raw archive APIs, but the product needs
   a typed portable manifest before restore/repair claims harden.

7. Add compatibility fixtures before layout epoch freeze.
   Fixture the current tree-cell key order, exact index tuple exclusion, reference
   and enum catalog-ID payloads, malformed metadata diagnostics, read-only opens,
   rollback, and commit metadata. Then freeze a layout epoch only after those
   fixtures pass.

8. Defer engine replacement until workload evidence exists.
   Keep redb as native v0.1. Re-evaluate RocksDB/LSM or SQLite-private-engine
   backends only when real workloads show B-tree/redb limits, and require the
   same typed tree-cell contract and backup manifest.
