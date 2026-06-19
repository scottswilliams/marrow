# Evaluator core

The runtime is a tree-walking interpreter directly over the checked AST: there is no separate IR or bytecode. `marrow-run` takes a `CheckedRuntimeProgram` (parsed and type-checked by `marrow-check`) plus a `TreeStore` and a `Host`, then interprets a named entry function by walking the `Checked*` nodes. Because the checker has already proven types, match coverage, and identity provenance, many runtime branches are defensive-only faults rather than reachable paths.

One run is a tree of activations. `run_entry` resolves the entry, builds an `Env`, and calls `invoke`, the single body-execution kernel. Each statement returns a `Flow`; each call spawns a child activation that ends in a `Completion`. Pure expressions produce a `Value`. Saved data is reached only through a `SavedPath` lowered from a checked place. Faults travel one channel: a `RuntimeError` that either carries an already-materialized language throw or lazily materializes a catchable runtime fault when a `catch` binds it.

## Parts

- **Entry and activation.** `run_entry*` unwraps one top-level invocation into a `RunOutput`. `invoke` binds module constants and read-only params, evaluates the body, and classifies the outcome into `Completion` (Returned/Threw/Faulted).
- **Dispatch.** `eval_call` routes every `CheckedCallTarget` variant — saved reads, constructors, builtins, std capabilities, local collections, program functions. `eval_expr` and `eval_statement` are the two recursive walkers everything flows through; sibling modules re-enter `eval_expr` for argument and key evaluation.
- **Control flow.** `Flow` (Normal/Return/ReturnAbsent/Break/Continue/Throw) is the result of a statement or block. `ReturnAbsent` is only produced by checked maybe-returning functions and becomes an absent call result, not a runtime `Value`. `eval_block` pushes/pops a scope balanced on every exit including faults; `loop_exec` maps innermost-loop Break/Continue to a `LoopStep`.
- **Values and saved data.** `Value` is the one owner of value-to-saved/key/leaf conversion and scalar-type classification. `SavedPath` (root, identity keys, layer chain, `Terminal`) is lowered once and is the consumption point for every saved read, write, delete, and exists.
- **Errors, host, and debugger facts.** `error` defines catchable-vs-fatal throw semantics and the stable `run.*` codes. `Host` is the capability gate (clock/env/log/filesystem/maintenance); `StepHook` is the opt-in debugger observer over a read-only `Frame`, and `Frame::debug_snapshot` captures bounded Marrow-owned debugger facts for stopped frames.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-run/src/lib.rs` | Crate root: declares submodules, re-exports the public surface (`run_entry*`, `RuntimeError` + `RUN_*` codes, `Host`/`Frame`/`StepHook`, `DebugFrameSnapshot`/`DebugValue*`, `Value`/`RunOutput`/`IdentityValue`, `WriteOp`/`WriteTarget`). |
| `crates/marrow-check/src/entry_abi.rs` | Checked entry ABI descriptors: builds `entry.invoke.v1` descriptor identities from public entry signatures, parameter shapes, accepted catalog identities, and return presence. The tag is a callable ABI tag, not a function-body digest. |
| `crates/marrow-run/src/entry.rs` | Public entry runtime API: `CheckedEntryCall::new` and `from_text_args` resolve text/dynamic entry args; `from_protocol_invocation` admits checker-owned entry descriptor identities and typed protocol values; `run_entry` / `run_entry_with_host` / `run_entry_with_debugger` drive one top-level call into a `RunOutput`. |
| `crates/marrow-run/src/activation.rs` | One call frame: `invoke` builds the `Env`, binds constants/params, runs the body, and classifies into `Completion`; `complete_call` re-raises that at the caller. |
| `crates/marrow-run/src/call.rs` | `eval_call` dispatches `CheckedCallTarget` variants; `function_by_ref` resolves module/function by index; `invoke_function` runs a child activation inheriting traversed layers and moving the debugger hook in and out. |
| `crates/marrow-run/src/call_args.rs` | Argument binding (positional/named/duplicate/missing), resource- and identity-constructor evaluation, `default_value` for uninitialized `var`. |
| `crates/marrow-run/src/expr.rs` | Pure-expression evaluation: literals, names/enum members, unary/binary operators with checked-overflow numeric and temporal arithmetic, string concatenation, comparison/equality, `is`, `??`, interpolation, field and optional-field dispatch. |
| `crates/marrow-run/src/statement.rs` | Statement execution: const/var binding, assignment dispatch, delete/return/throw/expr, control statements (if/match/break/continue/while/for/transaction/try); fires the before-statement hook. |
| `crates/marrow-run/src/exec.rs` | Block and match primitives: `eval_block` (balanced scope), `eval_statements` (stop at first non-Normal `Flow`), `eval_match` (enum-fact-driven, descendant matching), `local_target`. |
| `crates/marrow-run/src/loop_exec.rs` | Loops and iteration: `while`, `for` over ranges, single/two-name `for` over sequences, local trees, and streamed saved layers; `classify` maps `Flow` to `LoopStep`. |
| `crates/marrow-run/src/range_expr.rs` | Shared recognition of checked range expressions for loop execution and saved-range reads. |
| `crates/marrow-run/src/value.rs` | The `Value` model and its codecs: scalar/temporal/decimal/bytes/enum/sequence/local-tree/resource/identity, `IdentityValue` (root + keys), conversions to/from `SavedValue`/`SavedKey`/leaf bytes, text rendering. |
| `crates/marrow-run/src/env.rs` | The `Env` (scope stack, run context, output buffer, traversed layers, hook, depth), `Flow`/`Binding`/`Context`, scope/lookup/assign, loop-traversal write guards, transaction bookkeeping (`apply_plan`, open transaction depth, deferred required-field checks, commit metadata). |
| `crates/marrow-run/src/error.rs` | Fault model: `RuntimeError` (code/message/span + optional boxed throw value + catchable bit + `FileId` origin), the `RUN_*` constants, `raise`/`raise_fault`/`reraise_fault`/type/overflow constructors, the `Located` trait mapping `StoreError`/`ValueError` into spanned faults. |
| `crates/marrow-run/src/host.rs` | `Host` capability bundle (clock/env/log/filesystem/maintenance) with builders; `StepHook` (`before_statement` can abort, `before_write` is observational); the read-only `Frame` view and `Frame::debug_snapshot` boundary. |
| `crates/marrow-run/src/debugger.rs` | Bounded debugger facts: `DebugFrameSnapshot` plus `DebugValue*` previews, child counts, paged visible locals, truncation flags, one-based child labels, and zero-based page offsets. |
| `crates/marrow-run/src/path.rs` | Saved-path lowering: `lower` walks a checked place into a `SavedPath`; `SavedPath::read`/`write`, key lowering with identity-splice and typed-keyspace guards, `saved_path_present` for `exists`. |
| `crates/marrow-run/src/neighbor.rs` | `next`/`prev` ordered navigation: seeks the adjacent record or data-layer child via store cursors, returns an identity or key value, or an absent fault at a layer edge. |
| `crates/marrow-run/src/base64.rs` | The single canonical strict RFC 4648 base64 codec (padded encode, canonical-only decode) used by `std::bytes` builtins. |

Saved-write and read execution (`write*`, `read`, `durable_read`, `group_write`, `local_collection`, `saved_iter`), the stdlib boundary (`stdlib`, `std_pure`, `host_effects`), `transaction`, and `evolution` are sibling subsystems documented on their own pages.

## Invariants

- **One error channel.** Language throws ride `RuntimeError.throw` (a boxed Error `Value`) on the `Err` path, while runtime faults raised by `raise_fault` stay as code/message until a `catch` binds them. `catchable = false` means fatal/uncatchable. `raise_fault` keeps the dotted code if it escapes; `raise` relabels an uncaught language throw as `run.uncaught_error`.
- **`??` (`eval_coalesce`) only swallows catchable `RUN_ABSENT`.** Every other error, including fatal materialization-time absence for corrupt required saved data, propagates, so absence-default never hides a real fault; a `?.` chain short-circuits to the same absent fault.
- **Maybe function absence is typed at the call target.** `eval_call_expr` maps a `None` function result to catchable `RUN_ABSENT` only when the checked call target has `ReturnPresence::MaybePresent`; ordinary void calls still raise `run.no_value` in value position. No option-like user value represents absence.
- **Scopes balance on every exit, including faults**, so the `Env` is reusable after an error.
- **Origin stamping uses only-if-none**, so the deepest frame (the real raiser) wins as a fault unwinds; `FileId` origin is the only file identity a fault carries.
- **Debugger snapshots are bounded facts**, not raw runtime values: local and child pages carry counts/truncation flags, previews share the runtime preview helpers, and child labels are capped before values are captured.
- **Identities always carry checked root provenance**, including single-key ones, so a raw scalar is never accepted as an `Id(^store)` at dynamic or host boundaries. Every key crossing into the store is guarded against its declared scalar type.
- **Loop-traversal guards** dynamically reject a write that mutates a layer a loop is actively iterating (`run.traversal`), backing the static check for paths the checker cannot prove.
- **Maintenance-only operations** (drop a whole root, delete a lone required field) are gated by the `Host.maintenance` capability; an ordinary `marrow run` never sets it.

## Read next

- `crates/marrow-run/src/activation.rs` — `invoke` / `activation_completion`: where `Flow` becomes a `Completion` and origin/throw/fault classification lives.
- `crates/marrow-run/src/env.rs` — `Env::apply_plan` and the transaction methods: where evaluation meets durability (write-plan commit, traversal guards, open transaction depth, deferred required-field validation, commit-metadata stamping).
- `crates/marrow-run/src/path.rs` — `lower` / `SavedPath::read` / `SavedPath::write`: the one lowering pass every saved read, write, delete, and exists consumes.
- `crates/marrow-run/src/error.rs` — `raise` / `raise_fault` / `reraise_fault`: catchable-vs-fatal throw semantics and dotted-code preservation.
- `crates/marrow-run/src/call.rs` — `eval_call`: the central dispatcher for every call kind.
