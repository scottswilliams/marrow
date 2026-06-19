# Saved-data reads and ordered iteration

The read half of the runtime-to-store bridge. Given a type-checked saved place
or local collection, it resolves a physical store address, reads or decodes one
entry, or streams ordered iteration over record identities, index branches,
unique lookups, and keyed child layers. This page maps implementation ownership;
the user-facing iteration contract lives in `docs/language/builtins.md`.

One invariant organizes the whole subsystem: **durable saved data is never materialized as a `Value`.** Streaming over saved data happens only inside a `for` loop, which runs a `SavedLoopPlan`; calling `keys`/`values`/`entries`/`reversed` directly on a saved place is instead rejected with `durable_collection_value`, a `run.unsupported` fault. So unbounded store data is always iterated, never collected. See `collection.rs` `eval_values_materialized` / `durable_collection_value` (the rejection) and `saved_iter.rs` `SavedLoopSpec` (the streaming path).

## Parts

- **Point reads** (`durable_read.rs`): one scalar/optional field, one layer entry, one exact unique lookup, or whole-resource materialization of members.
- **Surface operations** (`surface.rs`): admitted singleton, point,
  collection-page, unique-index reads, sparse updates, and actions over stable
  `SurfaceFact`s. Production boundary callers can admit read, update, and action
  handles by stable operation tag instead of source IDs or read-operation
  ordinals. Node and collection reads pin a store snapshot for each
  materialization, page, or lookup; each returned record validates the full
  backing footprint required by the surface profile, then returns only the
  checked projection. The same module admits stable sparse updates over checked
  `SurfaceFact.update` fields and commits non-empty patches through managed
  write plans. Actions run ordinary public functions through `entry.invoke.v1`.
  Project-level linked-Rust surfaces enter through `ProjectSurfaceReadSession`
  or `ProjectSurfaceSession` in `project_session.rs`; both require an already
  accepted and stamped native store before admitting operation tags, with the
  write session exposing sparse updates and actions without exposing the store
  handle. The write session is a single-owner, sequential native writer; while
  it is open, another writer or read-only inspection handle cannot own the same
  native store.
- **Address resolution** (`read.rs`): classify an iterable path into Root / Index branch / ChildLayer; build node-backed record cursors plus index/data child cursors; count and probe presence without materializing.
- **Loop driver** (`saved_iter.rs` + four scan modules): the `ChildCursor` trait, the depth-bounded `walk_keyed_children`/`count_keyed_children` tree walk, the `LoopShape` row contract, and `SavedLoopSpec`/`SavedLoopPlan` that pick one of four scans (Root / Index / UniqueIndex / ChildLayer).
- **Builtins** (`collection.rs` + `collection/`): `keys`/`values`/`entries`/`reversed` dispatch, `Direction`, local materialization, and `append`/`nextId`.
- **Local collections** (`local_collection.rs`): in-memory `Sequence`/`LocalTree` that mirror the saved iteration contract.

## Module map

| File | Responsibility |
|---|---|
| `crates/marrow-run/src/read.rs` | Layer/index address resolution (`iterable_layer`, `iterable_index_branch`), record/index/data child-cursor primitives, identity/branch counting and presence, local field reads. |
| `crates/marrow-run/src/durable_read.rs` | Durable point reads: scalar field, optional field, layer-entry decode, exact unique-index lookup, whole-resource member materialization. |
| `crates/marrow-run/src/project_session.rs` | Project surface admission: `ProjectSurfaceReadSession` checks the project and opens the configured native store read-only for admitted reads; `ProjectSurfaceSession` opens an existing configured native store writable for admitted reads, sparse updates, and actions. Both require accepted catalog and store stamps and fence drift before admitting stable operation tags. |
| `crates/marrow-run/src/surface.rs` | Transport-neutral surface operations: store/catalog admission, stable operation-tag admission, fact-compiled projection and sparse-update plans, action admission over `entry.invoke.v1`, `surface.*` error mapping, snapshot-pinned singleton/point execution, collection pages, typed cursors, unique-index lookups, and managed sparse update execution. |
| `crates/marrow-run/src/saved_iter.rs` | Streaming loop driver: `ChildCursor`, `walk_keyed_children`/`count_keyed_children`, `LoopShape`/`shape_row`, `SavedLoopSpec`/`SavedLoopPlan`. |
| `crates/marrow-run/src/saved_iter/root.rs` | `RootScan` + `RecordCursor`: streams every record identity under a keyed root, reading the whole resource per shape. |
| `crates/marrow-run/src/saved_iter/index.rs` | `IndexScan` + `IndexCursor`: streams a non-unique index branch, exact-tuple paged scan (depth 0) or depth-bounded walk; `stream_exact_index_tuple`. |
| `crates/marrow-run/src/saved_iter/unique.rs` | `UniqueIndexScan`: yields at most one identity from a complete unique-index lookup. |
| `crates/marrow-run/src/saved_iter/child_layer.rs` | `ChildLayerScan`: streams keys of a keyed child layer (e.g. `^t(x).rows`) via data child cursors. |
| `crates/marrow-run/src/collection.rs` | `keys`/`values`/`entries`/`reversed` dispatch, `Direction`, `absent_read`, the no-materialize-durable rule. |
| `crates/marrow-run/src/collection/materialize.rs` | `values_or_entries`/`MaterializeKind`, `reversed_materialized`/`reversed_keys`: materialize local keyed collections, reject durable places. |
| `crates/marrow-run/src/collection/append.rs` | `eval_append`/`eval_next_id`: append to a local sequence or saved layer (read next free position, guard, plan+apply leaf write), mint next record id. |
| `crates/marrow-run/src/local_collection.rs` | In-memory `Sequence`/`LocalTree` read/write/count and ordered key/value/entry materialization mirroring the saved contract. |

## Key invariants

- **`shape_row` makes value reads lazy.** `read_value` runs only for Values/Entries shapes, so a Keys loop over a branch whose values are unsupported (e.g. a non-identity index column) succeeds and never decodes a record.
- **`Direction` reverses the whole walk uniformly.** Cursors flip first/next to last/prev at every level, so a composite identity is true-reversed at every component, not just the outermost.
- **Read-site resolution probes before reading.** `??`, `if const`, and `exists` test the fixed saved address first and treat ordinary absence as control flow. A saved read that reaches `SavedPath::read` and still finds absence raises fatal `run.absent_element`; required-field materialization after an address is fixed is the same invalid attached-data fault.
- **One tree-walk owner.** `walk_keyed_children` threads `query_prefix` and `identity_prefix` separately (index walks seek the full arg+identity prefix but yield only the identity suffix; record walks pass one slice for both); `count_keyed_children` reuses the same walk, folding a per-leaf count with `checked_add` and never paging. The depth-0 exact index tuple is paged at `INDEX_SCAN_PAGE_LIMIT` (128, defined in `read.rs`) in both the streaming scan (`saved_iter/index.rs`) and the counter (`read.rs` `count_exact_index_tuple`).
- **`SavedLoopPlan::run` pushes a `TraversedLayer`** for the streamed layer; `append` and writes call `guard_traversed_layer`, so mutating a layer mid-iteration faults rather than corrupting the walk.
- **`append` reads before it writes.** It computes `next_layer_pos` from the store tail, then plans and applies a leaf write at that 1-based position through `crate::write` (`plan_layer_leaf_write`) and `Env::apply_plan`; it lives in this read area because position allocation is fundamentally a tail read.
- **Local sequences are 1-based and dense; `LocalTree`s stay sorted on insert** so their enumeration matches saved ascending order. Composite local-tree keys enumerate only the first column.

## Read next

- `saved_iter.rs` — `walk_keyed_children` and `SavedLoopPlan::new`: the recursive depth-bounded walk and four-way scan selection; understand these and the four scan modules become thin.
- `read.rs` — `iterable_layer` / `iterable_index_branch`: how a checked path becomes a Root/Index/ChildLayer plan, with arg keys, `identity_start`, and walk depth.
- `durable_read.rs` — `read_layer_entry_at` and `read_resource` / `materialize_resource_members`: the single place an address becomes a `Value`, branching leaf-decode vs nested member materialization.
- `collection.rs` — `eval_values_materialized` / `durable_collection_value`: the saved-vs-local split and the no-materialize-durable invariant.
