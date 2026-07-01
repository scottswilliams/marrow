# Managed Writes

The write half of the runtime: every managed mutation is assembled in full, then
committed atomically. The statement evaluator dispatches each write kind to a
per-kind module that lowers the syntactic target to a checked `SavedPath`, builds
a `WritePlan` of typed `PlanStep`s over resolved store addresses, then commits
that plan through `TreeStore`. All type, identity, required-field, and
unique-index checks run during planning, so a `WriteError` aborts before any
cell changes. Generated-index teardown/rewrite and a catalog-epoch metadata
stamp fold into the same plan; the whole plan runs inside the active transaction
(or its own).

## The shape

`plan → commit`, never write-as-you-go. `write.rs` is the planning core for all write kinds; `write_plan.rs` defines the committable unit and the begin/apply/commit-or-rollback contract. The five per-kind modules under `write_dispatch/` plus `group_write.rs` are thin: lower the target, hand checked facts and a value to the planner, apply, defer the required-entry check. Plain assignment enters those modules with an unevaluated right-hand expression; a saved place evaluates it as an `Option<Value>` and routes the present arm to the write and the absent arm to the node-delete planner (present-or-clear), while compound assignment computes the value first and calls the sibling value-taking entries so the already-lowered target is not recomputed. `surface.rs` also uses the planner directly for admitted surface creates, sparse updates, and deletes. `transaction.rs` wraps multi-statement atomicity around language statements. `index_maintenance.rs` stages index steps into each plan, including combined field patches whose affected indexes must be rewritten from the final tuple rather than from one field at a time. `store.rs` is the only place checked names become physical store keys.

Step order is the correctness contract: within a plan, `DeleteData` (clear the old subtree) precedes `WriteRecordPresence` and per-field `WriteData`, and stale `DeleteIndex` precedes fresh `WriteIndex`, so a replace never leaves a stale field or index branch. Required-field enforcement is mode-split: outside a transaction the check rejects an incompletion before it lands; inside one it defers to the outermost commit, so an intermediate incomplete record is legal mid-transaction. Each deferred check carries the offending write's span paired with the remedy that fits how that write was made, so a commit-time `write.required_absent` points at the write that left the field unset rather than the `transaction` keyword, and its guidance matches the write kind: a field-by-field build is told to set the field before commit, a whole-value assignment to include it in the assigned value, and an ancestor left incomplete by a nested write to complete the containing record. The catalog-epoch stamp rides the same transaction as the data, and `WritePlan` resolves the stamp's commit ID against the predecessor metadata visible in that transaction. Transaction breadth is bounded the way depth is: `apply_plan` charges each in-transaction plan's real buffered footprint (value bytes, the staged key/path and index-key bytes, plus a calibrated per-staged-cell weight) to `TransactionState`, failing closed with catchable `write.transaction_too_large` once the running total crosses `TRANSACTION_WRITE_BYTE_BUDGET`, so an unbounded write set — large values or large composite keys alike — aborts before the in-memory buffer exhausts memory.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-run/src/write.rs` | Planning core: checked facts + value → `WritePlan` for every kind (resource, field, combined field patch, identity field, layer leaf/group, nested field, delete, root-drop); `nextId`/`nextLayerPos` allocation, type/identity/arity/required checks, field flattening. |
| `crates/marrow-run/src/write_plan.rs` | `PlanStep`, commit-id allocation policies, `WritePlan::commit()` against `TreeStore`, and read-only `WriteOp`/`WriteTarget` projection for dry-run/debug. |
| `crates/marrow-run/src/write_dispatch.rs` | Facade: declares the five per-kind submodules and re-exports their expression-taking and value-taking `eval_*`/`write_*` entry points. |
| `crates/marrow-run/src/write_dispatch/resource.rs` | Whole-record assign: flatten a `Resource` value into a `ResourceValue`, plan, apply, defer the entry check. |
| `crates/marrow-run/src/write_dispatch/field.rs` | Single saved-field write (top-level/nested, scalar/identity): present-or-clear — a present value plans/applies (immediate out-of-txn required check, note created-required paths), an absent `T?` routes to the field-delete planner so the node and its indexes are cleared like `delete ^p.f`. |
| `crates/marrow-run/src/write_dispatch/local.rs` | In-memory local-resource field set, descending and materializing unkeyed nested groups for a dotted path; no store contact. |
| `crates/marrow-run/src/write_dispatch/required.rs` | Required-field bookkeeping: newly-materialized fields, preexisting-data probes, required-path enumeration, checked-member predicates. |
| `crates/marrow-run/src/write_dispatch/delete.rs` | Delete dispatch: a `LocalCollection` call target removes a local sequence position or keyed entry (a hole or absent position is a tolerant no-op); otherwise field/nested/unkeyed-group/layer-entry/record/root, with required-field and maintenance-capability guards and maintenance delete notes. A saved address that resolves to no node — an absent position, including a non-positive sequence position — folds the catchable absent fault into a no-op. |
| `crates/marrow-run/src/group_write.rs` | Keyed-group-entry and keyed-leaf writes: lower record identity + parent layers + entry keys, branch leaf vs whole-entry, plan, apply. |
| `crates/marrow-run/src/transaction.rs` | User transaction block: open or join the flat transaction, run the body, then abort-and-discard on escape, or validate deferred entry checks + stamp metadata + commit at the outer boundary. |
| `crates/marrow-run/src/index_maintenance.rs` | Generated-index maintenance: stage delete-old + write-new index entries, stage delete-on-delete, reject unique conflicts via a 2-row scan, rewrite indexes from combined field patches, write identity-only indexes (keyed solely by identity, so no field write lists them) on each record-establishing write, rebuild an entry for evolution backfill. |
| `crates/marrow-run/src/store.rs` | Address layer: `DataAddress`/`LayerAddress`/`IndexAddress` resolve checked ids + identity + keys + member paths into store segments; thin `TreeStore` read wrappers. |

## Key types

- `WritePlan` / `PlanStep` (`write_plan.rs`) — ordered steps as the single committable unit. `PlanStep` covers `WriteRecordPresence`/`WriteData`/`DeleteData`/`DeleteRecordSubtree`, `WriteIndex`/`DeleteIndex`/`DeleteIndexSubtree`, and `StampMetadata`; it carries resolved addresses and commit-id allocation state, never source spelling.
- `ResourceValue` / `SuppliedIdentity` (`write.rs`) — flattened, type-resolved record value ready for planning.
- `DataAddress` / `LayerAddress` / `IndexAddress` (`store.rs`) — the boundary between checked names and physical store keys.
- `WriteError` (`write.rs`) — planning-stage failure with a stable `write.*` code; `env.apply_plan` turns it into a runtime fault. Codes are the contract, not the prose.

## Two things to know

- The rollback contract spans two files: `WritePlan::commit` owns single-statement atomicity; `transaction.rs` owns multi-statement atomicity plus deferred-check and metadata-stamp sequencing. Hold both to reason about ordering.
- `PlanStep::StampMetadata` is defined here. `env.rs` decides whether a managed write owes a stamp, while `write_plan.rs` reads predecessor commit metadata inside the active bracket and resolves `Baseline`, `Next`, or witness-pinned commit IDs at the stamp step.

## Read next

- `write.rs` → `plan_resource_write` — the canonical plan shape: identity resolution, required-field collection, unique rejection, `DeleteData`-then-`WriteData` ordering, index-rewrite folding.
- `write.rs` → `plan_resource_write`, `plan_field_patch_write`, and
  `plan_resource_delete` — the planner entries used by generated surface
  create, update, and delete operations.
- `write_plan.rs` → `WritePlan::commit` / `apply_steps` — the exact begin/apply/commit-or-rollback contract and each `PlanStep` → `TreeStore` mapping.
- `transaction.rs` → `eval_transaction` — deferred entry validation, commit-metadata stamping, store commit, discard-on-failure.
- `index_maintenance.rs` → `stage_resource_index_rewrites` / `check_unique_conflict` — how teardown+rewrite is staged and how unique conflicts are detected ignoring self-identity.
- `index_maintenance.rs` → `index_rebuild_entry_with_staged` — the entry reused by evolution backfill to rebuild index entries from transaction-visible stored data.
