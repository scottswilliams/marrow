# Runtime (marrow-run)

The runtime is the final pipeline stage. It takes a checked project session or a lower-level `CheckedRuntimeProgram`, a `TreeStore`, and a `Host`, and runs a named entry function. The no-host convenience entry builds `Host::new()`, which grants no capabilities; explicit-host entry points receive the caller's capability bundle. There is no bytecode or separate IR: the runtime is a tree-walking interpreter directly over the checked AST, so the `Checked*` types *are* the executable form and the checker's proofs make many runtime branches defensive-only.

## The shared model

- **Values.** Everything flows as `Value` (`value.rs`): scalars, temporals, decimals, bytes, enums, sequences, local trees, resources, and identities. `Value` is the single owner of conversion to and from store `SavedValue`/`SavedKey`/leaf bytes and of scalar-type classification.
- **One error channel.** `RuntimeError` (`error.rs`) carries an optional boxed throw `Value` plus a catchable bit, so an expression-position throw and a statement-position `Flow::Throw` (`env.rs`) converge on one mechanism. Runtime faults raised by `raise_fault` materialize an `Error` value only when a `catch` binds them. `catchable = false` is fatal; an uncaught language throw surfaces as `run.uncaught_error`, an uncaught fault keeps its dotted `run.*` code.
- **Activations.** Each call is an `invoke` (`activation.rs`) that builds an `Env`, binds constants and params, evaluates the body, and classifies the outcome into a `Completion` (Returned / Threw / Faulted). Transaction state is shared (`Rc<RefCell>`) across all activations so callee writes join the caller's open transaction.
- **The store bridge.** Reads, writes, and `exists` go through one saved-path lowering pass: `lower` produces a `SavedPath` with a `Terminal` (`path.rs`), and every read/write/delete consumes it. Writes are always **plan-then-commit**: build a full `WritePlan` of typed `PlanStep`s, then commit atomically — no write-as-you-go.

## Run order

1. `ProjectSession::open` (`project_session.rs`) checks a project for `run` or `test`, binds catalog identity for the selected mode, and selects the run store policy: configured-store admission through the activation fence, isolated dry-run admission, or a fresh in-memory store that admits no configured store.
2. `ProjectSession::invoke` builds a `CheckedEntryCall` and selects the admitted run store or a fresh test store, then calls `run_entry*` (`entry.rs`) to resolve the entry, canonicalize and type-check args, and start the top activation.
3. `eval_call` (`call.rs`) dispatches every saved read, constructor, builtin, std capability, local-collection, and program-function call.
4. `eval_statement` / `eval_expr` (`statement.rs`, `expr.rs`) walk the body; saved reads stream through the read bridge, saved writes build and commit plans, stdlib calls branch on the checker-stamped `Capability`.
5. Evolution admission for `run` lives in `project_session.rs`: the session freezes a pending baseline, fences on `(source_digest, accepted_epoch, engine_profile)`, auto-applies zero-record-mutation drift through the production apply path, and refuses unstamped populated stores before invocation.

## The six areas

| Area | Spine | One-line responsibility |
| --- | --- | --- |
| Project sessions | `project_session.rs` | Load and check run/test projects, bind catalog identity, admit configured stores through the activation fence or select fresh memory, and invoke entries through one session path. |
| [Evaluator core](evaluator.md) | `entry.rs`, `activation.rs`, `call.rs`, `expr.rs`, `statement.rs`, `exec.rs`, `loop_exec.rs`, `env.rs`, `error.rs`, `host.rs`, `path.rs` | Walk the checked AST: values, control flow, calls, loops, the error channel, the host boundary, and saved-path lowering. |
| [Reads and iteration](saved-data.md) | `read.rs`, `durable_read.rs`, `saved_iter.rs` (+ `saved_iter/`), `collection.rs`, `local_collection.rs` | Resolve a checked place to a store address; decode one entry or stream ordered iteration for `for`/`keys`/`values`/`entries`/`count`. Durable data is never materialized as a `Value`. |
| Surface node reads | `surface.rs` | Admit a stable checked surface against a stamped store and execute the backing singleton or point read with full-record validation and projection-only output. |
| [Managed writes](writes.md) | `write.rs`, `write_plan.rs`, `write_dispatch/`, `group_write.rs`, `transaction.rs`, `index_maintenance.rs`, `store.rs` | Lower a write target to a `SavedPath`, build a typed `WritePlan` (data + generated indexes + metadata stamp), and commit it atomically inside the active transaction. |
| [Evolution](evolution.md) | `evolution/` (`apply.rs`, `auto_apply.rs`, `backfill.rs`, `transform.rs`, `window.rs`, ...) | Consume the read-only `EvolutionWitness`, re-validate it byte-for-byte, fence, gate, re-derive every backfill/transform/rebuild/retire from the live store, and commit the writes plus slim stamp in one transaction. |
| [Standard library](stdlib.md) | `std_pure.rs`, focused `std_*` modules, `host_effects.rs`, `stdlib/` (`args.rs`, `conversion.rs`, `count.rs`, `error_constructor.rs`, `index_lookup.rs`, `output.rs`, ...) | Evaluate a checker-resolved `std::*` op or builtin: pure helpers compute in focused modules; host-effect helpers read a `Host` capability (`Option` for clock/context/env/log, the `bool` `filesystem` for io) and raise `run.capability` when absent. |

The runtime never re-resolves names or re-parses op strings: the checker stamps every call target, capability, and conversion kind, so each area branches on a typed kind, not on source spelling. The single std descriptor table lives in `marrow-schema::stdlib`; physical store addresses are built only in `store.rs`.

## Read next

- `project_session.rs` — `ProjectSession::open` / `ProjectSession::invoke`: the project admission and invocation boundary.
- `entry.rs` — `run_entry` / `CheckedEntryCall::new`: how one admitted entry starts.
- `activation.rs` — `invoke`: the body-execution kernel and `Completion` classification.
- `call.rs` — `eval_call`: the central dispatcher every call routes through.
- `path.rs` — `lower` / `SavedPath::read` / `SavedPath::write`: the one saved-path pass behind every data feature.
- `error.rs` — `raise` / `raise_fault`: catchable-vs-fatal throw semantics.
