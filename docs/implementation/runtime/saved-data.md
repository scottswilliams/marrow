# Saved-data reads and ordered iteration

The read half of the runtime-to-store bridge. Given a type-checked saved place
or local collection, it resolves a physical store address, reads or decodes one
entry, or streams ordered iteration over record identities, index branches,
unique lookups, and keyed child layers. This page maps implementation ownership;
the user-facing iteration contract lives in `docs/language/builtins.md`.

One invariant organizes the whole subsystem: **durable saved data is never materialized as a `Value`.** Streaming over saved data happens only inside a `for` loop, which runs a `SavedLoopPlan`; the `keys`/`values` builtins materialize a *local* collection and reject a saved place with `durable_collection_value`, a `run.unsupported` fault. So unbounded store data is always iterated, never collected. See `collection.rs` `eval_values_materialized` / `durable_collection_value` (the rejection) and `saved_iter.rs` `SavedLoopSpec` (the streaming path).

## Parts

- **Point reads** (`durable_read.rs`): one scalar/optional field, one layer entry, one exact unique lookup, or whole-resource materialization of members.
- **Surface operations** (`surface.rs`): admitted singleton, point,
  collection-page, unique-index reads, exact-body creates, sparse updates,
  full-subtree deletes, and actions over stable `SurfaceFact`s. Production
  boundary callers can admit read, create, update, delete, and action handles by
  stable operation tag instead of source IDs or read-operation
  ordinals. Node and collection reads pin a store snapshot for each
  materialization, page, or lookup; each returned record validates the full
  backing footprint required by the surface profile, then returns only the
  checked projection. The same module admits stable creates over checked
  `SurfaceFact.create` fields, sparse updates over checked `SurfaceFact.update`
  fields, and deletes over checked `SurfaceFact.delete`, committing each
  generated write through managed write plans. Actions run ordinary public
  functions through `entry.invoke.v1`. Computed reads run ordinary public
  read-only functions through the same checked entry invocation path.
  Project-level linked-Rust surfaces enter through `ProjectSurfaceReadSession`
  or `ProjectSurfaceSession` in `project_session.rs`; both require an already
  accepted and stamped native store before admitting operation tags, with the
  read session exposing reads and computed reads, and the write session exposing
  reads, computed reads, create/update/delete/action execution without exposing
  the store handle. The write session is a single-owner, sequential native
  writer; while it is open, another writer or read-only inspection handle cannot
  own the same native store.
- **Address resolution** (`read.rs`): classify an iterable path into Root / Index branch / ChildLayer; build node-backed record cursors plus index/data child cursors; count and probe presence without materializing.
- **Loop driver** (`saved_iter.rs` + four scan modules): the `ChildCursor` trait, the depth-bounded `walk_keyed_children`/`count_keyed_children` tree walk, the `SavedLoopRow` (`Key`/`Full`) row contract, and `SavedLoopSpec`/`SavedLoopPlan` that pick one of four scans (Root / Index / UniqueIndex / ChildLayer). The head's key-column count and value flag ride on the spec; traversal direction is the head `reversed` keyword mapped to `Direction`.
- **Builtins** (`collection.rs` + `collection/`): `keys`/`values` local materialization, `count`, `Direction`, and `append`/`nextId`.
- **Local collections** (`collection/local.rs`): the in-memory `Sequence`/`LocalTree` kernel, addressed by the validated `Position`/`CollectionKey` newtypes minted at this one boundary; mirrors the saved iteration contract.

## Module map

| File | Responsibility |
|---|---|
| `crates/marrow-run/src/read.rs` | Layer/index address resolution (`iterable_layer`, `iterable_index_branch`), record/index/data child-cursor primitives, identity/branch counting and presence, local field reads. |
| `crates/marrow-run/src/durable_read.rs` | Durable point reads: scalar field, optional field, layer-entry decode, exact unique-index lookup, whole-resource member materialization. |
| `crates/marrow-run/src/project_session.rs` | Project surface admission: `ProjectSurfaceReadSession` checks the project and opens the configured native store read-only for admitted reads and computed reads; `ProjectSurfaceSession` opens an existing configured native store writable for admitted reads, computed reads, generated writes, and actions. Both require accepted catalog and store stamps and fence drift before admitting stable operation tags. |
| `crates/marrow-run/src/surface.rs` | Transport-neutral surface operations: store/catalog admission, stable operation-tag admission, fact-compiled projection and generated write plans, computed-read and action admission over checked entry invocation descriptors, `surface.*` error mapping, snapshot-pinned singleton/point execution, collection pages, typed cursors, unique-index lookups, and managed create/update/delete execution. |
| `crates/marrow-run/src/saved_iter.rs` | Streaming loop driver: `ChildCursor`, `walk_keyed_children`/`count_keyed_children`, `SavedLoopRow`/`saved_loop_row`, `SavedLoopSpec`/`SavedLoopPlan`. |
| `crates/marrow-run/src/saved_iter/root.rs` | `RootScan` + `RecordCursor`: streams every record identity under a keyed root, reading the whole resource per shape. |
| `crates/marrow-run/src/saved_iter/index.rs` | `IndexScan` + `IndexCursor`: streams a non-unique index branch by delegating to `read.rs` `stream_index_branch`; every yield is a store identity. |
| `crates/marrow-run/src/saved_iter/unique.rs` | `UniqueIndexScan`: yields at most one identity from a complete unique-index lookup. |
| `crates/marrow-run/src/saved_iter/child_layer.rs` | `ChildLayerScan`: streams a keyed child layer's key columns via data child cursors. A composite layer is a chain of single-key sub-layers, so a key-first single binding streams the outer column while an (n+1)-name head walks every remaining column depth-first; a partial-key prefix pins the exact leading keys and a trailing range bounds a single streamed column. |
| `crates/marrow-run/src/collection.rs` | `keys`/`values` local materialization dispatch, `Direction`, `absent_read` (catchable `run.absent_element` a below-1 sequence position raises at write/lowering time), the no-materialize-durable rule. |
| `crates/marrow-run/src/collection/append.rs` | `eval_append`/`eval_next_id`: append to a local sequence or saved layer (read next free position, guard, plan+apply leaf write), mint next record id. |
| `crates/marrow-run/src/collection/local.rs` | The one boundary that mints a `Position`/`CollectionKey` address for the in-memory `Sequence`/`LocalTree` kernel: read/write/delete/count and ordered key/value materialization mirroring the saved contract. |

## Key invariants

- **`saved_loop_row` makes value reads lazy.** `read_value` runs only when the head binds a value (a `Full` row), so a key-only head pays nothing to materialize the record it would otherwise read.
- **A non-unique index branch always yields the store identity.** Any partial prefix (bare, single field, or down to the identity suffix) streams identities; `stream_index_branch` walks each branch to the leaf and slices the trailing `identity_start..` suffix. An enum-typed component is stored as a content-independent member id, which does not sort by ordinal, so the walked enum level — whether bare or bounded by a range — resolves through `IndexRange::EnumMembers` and scans the (in-range) members as exact prefixes in declaration order, failing closed when a physical child is not a declared member; scalar components scan an order-preserving key-byte range.
- **`Direction` reverses the whole walk uniformly.** Cursors flip first/next to last/prev at every level, so a composite identity is true-reversed at every component, not just the outermost.
- **Read-site resolution probes before reading.** `??`, `if const`, `exists`, and materializing a maybe-present saved read into a `T?` slot (a `const`/`var`/argument binding or `return`, via `eval_into_slot`) test the fixed saved address first via `read_saved_value_if_present`, yielding `None` — the empty optional — for ordinary absence. A saved read that reaches `SavedPath::read` and still finds absence raises fatal `run.absent_element`; a non-optional (required or narrowed-present) read materialized through `eval_expr` after an address is fixed is that same invalid attached-data fault. A local-collection indexed read, a sparse field of a materialized value, a stdlib cell selection, and a `next`/`prev` layer edge have no fixed saved address to probe, so they evaluate to the empty optional (`Value::Absent`), which `eval_optional` collapses to `None` at the resolution site.
- **A non-positive sequence position addresses no node on every read path.** Lowering a below-1 position raises the catchable absent fault; a write surfaces it, while `eval_local_collection_read` maps it to the empty optional so a read resolves it. `count` and the `next`/`prev` neighbor seek lower through `lower_for_probe`: a folded-away position counts as 0 and seeks no neighbor, exactly as a positive out-of-range position does.
- **One tree-walk owner.** `walk_keyed_children_after` threads `query_prefix` and `identity_prefix` separately and returns the visitor's `ControlFlow<Flow>` unchanged; an index walk passes its exact prefix as both, yields the full index tuple, then slices the identity. Preserving the `ControlFlow` lets a body `break` stop `stream_index_branch`'s enum-member loop instead of bleeding into the next member. `walk_keyed_children` is the `Flow`-collapsing wrapper used where there is no surrounding member loop; `count_keyed_children` reuses it, folding a per-leaf count with `checked_add` and never paging. `read.rs` `stream_index_branch` is the single owner of branch iteration for the loop, count, and presence paths; a fully pinned tuple is read as an exact entry paged at `INDEX_SCAN_PAGE_LIMIT` (128).
- **`SavedLoopPlan::run` pushes a `TraversedLayer`** for the streamed layer; `append` and writes call `guard_traversed_layer`, so mutating a layer mid-iteration faults rather than corrupting the walk.
- **`append` reads before it writes.** It computes `next_layer_pos` from the store tail, then plans and applies a leaf write at that 1-based position through `crate::write` (`plan_layer_leaf_write`) and `Env::apply_plan`; it lives in this read area because position allocation is fundamentally a tail read.
- **Local sequences are 1-based; both `Sequence` and `LocalTree` are key-ordered maps** so both enumerate in saved ascending order. `Sequence` backs the local sequence with a `BTreeMap` from position to value and `LocalTree` backs the keyed local tree with a `BTreeMap` from the full key tuple to its value, keeping insert/lookup/delete `O(log n)` for any position or key arrival order. Composite local-tree keys enumerate only the first column. A position below 1 never becomes a `Position`, so an out-of-range sequence address is unrepresentable rather than guarded by convention.

## Read next

- `saved_iter.rs` — `walk_keyed_children` and `SavedLoopPlan::new`: the recursive depth-bounded walk and four-way scan selection; understand these and the four scan modules become thin.
- `read.rs` — `iterable_layer` / `iterable_index_branch` / `stream_index_branch`: how a checked path becomes a Root/Index/ChildLayer plan, with arg keys, `identity_start`, `walk_depth`, and the scalar/enum range split.
- `durable_read.rs` — `read_layer_entry_at` and `read_resource` / `materialize_resource_members`: the single place an address becomes a `Value`, branching leaf-decode vs nested member materialization.
- `collection.rs` — `eval_values_materialized` / `durable_collection_value`: the saved-vs-local split and the no-materialize-durable invariant.
