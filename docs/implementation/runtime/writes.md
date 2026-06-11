# Managed Writes

The write half of the runtime: every managed mutation is planned in full, then committed atomically. The statement evaluator dispatches each write kind to a per-kind module that lowers the syntactic target to a checked `SavedPath`, builds a `WritePlan` of typed `PlanStep`s over resolved store addresses, then commits that plan through `TreeStore`. All type, identity, required-field, and unique-index checks run during planning, so a `WriteError` aborts before any cell changes. Generated-index teardown/rewrite and a catalog-epoch metadata stamp fold into the same plan; the whole plan runs inside the active transaction savepoint (or its own).

## The shape

`plan → commit`, never write-as-you-go. `write.rs` is the planning core for all write kinds; `write_plan.rs` defines the committable unit and the begin/apply/commit-or-rollback contract. The five per-kind modules under `write_dispatch/` plus `group_write.rs` are thin: lower the target, hand checked facts and a value to the planner, apply, defer the required-entry check. `transaction.rs` wraps multi-statement atomicity around that. `index_maintenance.rs` stages index steps into each plan. `store.rs` is the only place checked names become physical store keys.

Step order is the correctness contract: within a plan, `DeleteData` (clear the old subtree) precedes `WriteNode` and per-field `WriteData`, and stale `DeleteIndex` precedes fresh `WriteIndex`, so a replace never leaves a stale field or index branch. Required-field enforcement is mode-split: outside a transaction the check rejects an incompletion before it lands; inside one it defers to commit (validated at depth 1), so an intermediate incomplete record is legal mid-transaction. The catalog-epoch stamp rides the same transaction as the data, so the epoch never advances without the data it describes.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-run/src/write.rs` | Planning core: checked facts + value → `WritePlan` for every kind (resource, field, identity field, layer leaf/group, nested field, delete, root-drop); `nextId`/`nextLayerPos` allocation, type/identity/arity/required checks, field flattening. |
| `crates/marrow-run/src/write_plan.rs` | `PlanStep`, `WritePlan`, and `commit()` against `TreeStore`; read-only `WriteOp`/`WriteTarget` projection for dry-run/debug. |
| `crates/marrow-run/src/write_dispatch.rs` | Facade: declares the five per-kind submodules and re-exports their `eval_*`/`write_*` entry points. |
| `crates/marrow-run/src/write_dispatch/resource.rs` | Whole-record assign: flatten a `Resource` value into a `ResourceValue`, plan, apply, defer the entry check. |
| `crates/marrow-run/src/write_dispatch/field.rs` | Single saved-field write (top-level/nested, scalar/identity): plan, immediate out-of-txn required check, apply, note created-required paths. |
| `crates/marrow-run/src/write_dispatch/local.rs` | In-memory local-resource field set; no store contact. Shared by field-set syntax and `inout` write-back. |
| `crates/marrow-run/src/write_dispatch/required.rs` | Required-field bookkeeping: newly-materialized fields, preexisting-data probes, required-path enumeration, checked-member predicates. |
| `crates/marrow-run/src/write_dispatch/delete.rs` | Delete dispatch: field/nested/unkeyed-group/layer-entry/record/root; required-field and maintenance-capability guards; maintenance delete notes. |
| `crates/marrow-run/src/group_write.rs` | Keyed-group-entry and keyed-leaf writes: lower record identity + parent layers + entry keys, branch leaf vs whole-entry, plan, apply. |
| `crates/marrow-run/src/transaction.rs` | User transaction block: savepoint open, body, then rollback-and-discard on throw, or validate deferred entry checks + stamp metadata + commit. |
| `crates/marrow-run/src/index_maintenance.rs` | Generated-index maintenance: stage delete-old + write-new index entries, stage delete-on-delete, reject unique conflicts via a 2-row scan, rebuild an entry for evolution backfill. |
| `crates/marrow-run/src/store.rs` | Address layer: `DataAddress`/`LayerAddress`/`IndexAddress` resolve checked ids + identity + keys + member paths into store segments; thin `TreeStore` read wrappers. |

## Key types

- `WritePlan` / `PlanStep` (`write_plan.rs`) — ordered steps as the single committable unit. `PlanStep` covers `WriteNode`/`WriteData`/`DeleteData`/`DeleteRecordSubtree`, `WriteIndex`/`DeleteIndex`/`DeleteIndexSubtree`, and `StampMetadata`; it carries resolved addresses, never source spelling.
- `ResourceValue` / `SuppliedIdentity` (`write.rs`) — flattened, type-resolved record value ready for planning.
- `DataAddress` / `LayerAddress` / `IndexAddress` (`store.rs`) — the boundary between checked names and physical store keys.
- `WriteError` (`write.rs`) — planning-stage failure with a stable `write.*` code; `env.apply_plan` turns it into a runtime fault. Codes are the contract, not the prose.

## Two things to know

- The rollback contract spans two files: `WritePlan::commit` owns single-statement (own-savepoint) atomicity; `transaction.rs` owns multi-statement atomicity plus deferred-check and metadata-stamp sequencing. Hold both to reason about ordering.
- `PlanStep::StampMetadata` is defined here, but the stamp is *computed* in `env.rs` (`stamp_managed_write` / `build_commit_metadata_stamp` via `crate::evolution::metadata_stamp`) and folded in by `Env::apply_plan` / `Env::stamp_transaction_commit`, not by these files.

## Read next

- `write.rs` → `plan_resource_write` — the canonical plan shape: identity resolution, required-field collection, unique rejection, `DeleteData`-then-`WriteData` ordering, index-rewrite folding.
- `write_plan.rs` → `WritePlan::commit` / `apply_steps` — the exact begin/apply/commit-or-rollback contract and each `PlanStep` → `TreeStore` mapping.
- `transaction.rs` → `eval_transaction` — deferred entry validation, commit-metadata stamping, store commit, discard-on-failure.
- `index_maintenance.rs` → `stage_resource_index_rewrites` / `check_unique_conflict` — how teardown+rewrite is staged and how unique conflicts are detected ignoring self-identity.
- `index_maintenance.rs` → `index_rebuild_entry_with_staged` — the entry reused by evolution backfill to rebuild index entries from stored-or-staged data.
