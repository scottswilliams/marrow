# Runtime (marrow-run)

The runtime is the final pipeline stage. It takes a `CheckedRuntimeProgram` (already parsed and type-checked by marrow-check), a `TreeStore`, and an optional `Host`, and runs a named entry function. There is no bytecode or separate IR: the runtime is a tree-walking interpreter directly over the checked AST, so the `Checked*` types *are* the executable form and the checker's proofs make many runtime branches defensive-only.

## The shared model

- **Values.** Everything flows as `Value` (`value.rs`): scalars, temporals, decimals, bytes, enums, sequences, local trees, resources, and identities. `Value` is the single owner of conversion to and from store `SavedValue`/`SavedKey`/leaf bytes and of scalar-type classification.
- **One error channel.** `RuntimeError` (`error.rs`) carries an optional boxed throw `Value` plus a catchable bit, so an expression-position throw and a statement-position `Flow::Throw` (`env.rs`) converge on one mechanism. Runtime faults raised by `raise_fault` materialize an `Error` value only when a `catch` binds them. `catchable = false` is fatal; an uncaught language throw surfaces as `run.uncaught_error`, an uncaught fault keeps its dotted `run.*` code.
- **Activations.** Each call is an `invoke` (`activation.rs`) that builds an `Env`, binds constants and params, evaluates the body, and classifies the outcome into a `Completion` (Returned / Threw / Faulted). Transaction state is shared (`Rc<RefCell>`) across all activations so callee writes join the caller's open transaction.
- **The store bridge.** Reads, writes, and `exists` go through one saved-path lowering pass: `lower` produces a `SavedPath` with a `Terminal` (`path.rs`), and every read/write/delete consumes it. Writes are always **plan-then-commit**: build a full `WritePlan` of typed `PlanStep`s, then commit atomically — no write-as-you-go.

## Phase order of one run

1. `run_entry*` (`entry.rs`) resolves the entry, canonicalizes and type-checks args, and starts the top activation.
2. `eval_call` (`call.rs`) dispatches every saved read, constructor, builtin, std capability, local-collection, and program-function call.
3. `eval_statement` / `eval_expr` (`statement.rs`, `expr.rs`) walk the body; saved reads stream through the read bridge, saved writes build and commit plans, stdlib calls branch on the checker-stamped `Capability`.
4. Evolution gating wraps a run but lives in the `marrow` CLI, not the runtime: `marrow run` (`cmd_run.rs`) calls `fence` and, on store drift, `try_auto_apply` before `run_entry`; `marrow evolve apply` (`cmd_evolve`) commits the witness's durable rewrite. The runtime exports these from `evolution/`, but `run_entry` never calls them.

## The five areas

| Area | Spine | One-line responsibility |
| --- | --- | --- |
| [Evaluator core](evaluator.md) | `entry.rs`, `activation.rs`, `call.rs`, `expr.rs`, `statement.rs`, `exec.rs`, `loop_exec.rs`, `env.rs`, `error.rs`, `host.rs`, `path.rs` | Walk the checked AST: values, control flow, calls, loops, the error channel, the host boundary, and saved-path lowering. |
| [Reads and iteration](saved-data.md) | `read.rs`, `durable_read.rs`, `saved_iter.rs` (+ `saved_iter/`), `collection.rs`, `local_collection.rs` | Resolve a checked place to a store address; decode one entry or stream ordered iteration for `for`/`keys`/`values`/`entries`/`count`. Durable data is never materialized as a `Value`. |
| [Managed writes](writes.md) | `write.rs`, `write_plan.rs`, `write_dispatch/`, `group_write.rs`, `transaction.rs`, `index_maintenance.rs`, `store.rs` | Lower a write target to a `SavedPath`, build a typed `WritePlan` (data + generated indexes + metadata stamp), and commit it atomically inside the active transaction. |
| [Evolution](evolution.md) | `evolution/` (`apply.rs`, `auto_apply.rs`, `backfill.rs`, `transform.rs`, `window.rs`, `completion/`, ...) | Consume the read-only `EvolutionWitness`, re-validate it byte-for-byte, fence, gate, re-derive every backfill/transform/rebuild/retire from the live store, and commit one atomic plan. |
| [Standard library](stdlib.md) | `std_pure.rs`, `host_effects.rs`, `stdlib/` (`args.rs`, `conversion.rs`, `count.rs`, `error_constructor.rs`, `index_lookup.rs`, `output.rs`, ...) | Evaluate a checker-resolved `std::*` op or builtin: pure helpers compute in place; host-effect helpers read a `Host` capability (`Option` for clock/env/log, the `bool` `filesystem` for io) and raise `run.capability` when absent. |

The runtime never re-resolves names or re-parses op strings: the checker stamps every call target, capability, and conversion kind, so each area branches on a typed kind, not on source spelling. The single std descriptor table lives in `marrow-schema::stdlib`; physical store addresses are built only in `store.rs`.

## Read next

- `entry.rs` — `run_entry` / `CheckedEntryCall::new`: how a run starts.
- `activation.rs` — `invoke`: the body-execution kernel and `Completion` classification.
- `call.rs` — `eval_call`: the central dispatcher every call routes through.
- `path.rs` — `lower` / `SavedPath::read` / `SavedPath::write`: the one saved-path pass behind every data feature.
- `error.rs` — `raise` / `raise_fault`: catchable-vs-fatal throw semantics.
