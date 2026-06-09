# Presence and effect analysis

A post-typecheck static pass over the lowered runtime IR that proves, before runtime, that every read of maybe-present saved data is justified. It walks each function and constant body flow-sensitively, tracking *narrowings* — places proven present by an `if exists(...)` guard, a required-slot declaration, or a unique store-index lookup — emits one `PresenceProofFact` per saved read, and raises `CHECK_BARE_MAYBE_PRESENT_READ` when a maybe-present read is reached unguarded. A second body-local entry summarizes each block's saved reads/writes, host capabilities, throws, transactions, and user-function calls into `DirectEffectFacts`.

The pass runs near the end of `analyze_source_project` (`analysis.rs`, after lowering runtime bodies and the evolution transform-effects check). It mutates `program.facts` and pushes diagnostics; it owns no store access of its own.

## The big idea

Narrowing identity is by **span-stripped canonical key**, never by structural `CheckedExpr` equality. Two textually equal reads carry different spans, so `keys.rs` folds each read to a tagged string plus the scope binding ids it reads. A narrowing keyed on binding ids expires when any read binding is reassigned (or an `inout` arg is mutated), when an overlapping saved write occurs (overlap is by key/member prefix in either direction), or when a branch it lived in joins back. Branch narrowings work on a cloned narrowed set and never escape to the join point. Effect identity always uses stable schema ids (`SavedPlaceEffect` = `ResourceId` + `ResourceMemberId` path); an unresolvable path yields no proof rather than a string-keyed one.

## Parts

- **Flow driver** (`walk.rs`): threads the narrowed set and `NameScope`, classifies each read's `ReadContext`, dispatches builtins, records proofs.
- **Narrowing algebra** (`effects.rs`): what `exists`/`&&` and loop traversals narrow, and the invalidation rules that expire narrowings.
- **Canonical key** (`keys.rs`): the one owner of the span-free key format.
- **Read resolution** (`target.rs`): expression to `ReadTarget`/`ReadPlace`, to a persisted `PresenceProofPlace`, and the required-slot presence test.
- **Proofs** (`proofs.rs`): the only place the bare-maybe-present diagnostic is raised; maps context to proof source/status and records the fact.
- **Direct effects** (`direct.rs`) and **saved-write reachability** (`writes.rs`, cycle-guarded across user functions).

## Modules

| File | Responsibility |
| --- | --- |
| `presence.rs` | Module root; defines `check_presence` as a thin wrapper over `walk::check_presence` and re-exports `direct_effects_for_block` and `read_target`. |
| `presence/walk.rs` | Flow-sensitive driver over bodies and statements; threads narrowed set + scope, classifies reads, dispatches builtins, invalidates on writes and branch joins. |
| `presence/direct.rs` | Body-local effect collector producing `DirectEffectFacts` for one block without expanding callee effects. |
| `presence/effects.rs` | Narrowing algebra: condition/loop narrowings and the invalidation (key-binding, written-target overlap, removed-on-branch, saved-wipe) rules. |
| `presence/keys.rs` | Sole owner of the canonical span-stripped narrowing key; extracts `SavedPathParts` from a saved path. |
| `presence/target.rs` | Resolves an expression to a `ReadTarget`/`ReadPlace`, maps to a `PresenceProofPlace`, decides required-slot proof. |
| `presence/writes.rs` | Recursive saved-write reachability through callee bodies, reading each function's precomputed `direct_effects.saved_writes`. |
| `presence/proofs.rs` | Builds a `ReadProof`, assigns source/status, records the fact, emits the bare-maybe-present diagnostic. |
| `presence/calls.rs` | Typed-call helpers: std Path-argument mask, neighbor read direction, single-arg collection-view unwrap. |
| `presence/scope.rs` | `NameScope`: frame stack mapping names to monotonic binding ids. |
| `presence/util.rs` | `push_unique`/`extend_unique` dedup helpers for narrowing/binding lists. |

Key types live mostly in `presence/target.rs` (`ReadTarget`, `ReadPlace`), `presence/keys.rs` (`ExprKey`, `SavedPathParts`), and `presence/proofs.rs` (`ReadContext`, `ReadProof`). The persisted forms — `DirectEffectFacts`, `PresenceProofFact`/`PresenceProofDraft`, `SavedPlaceEffect` — live in `facts.rs`.

## Entry points

| Symbol | Caller | Role |
| --- | --- | --- |
| `check_presence` | `analysis.rs` (after lowering) | Runs the flow-sensitive walk, mutating facts and pushing diagnostics. |
| `direct_effects_for_block` | `facts.rs` `refresh_direct_effects`, `evolution/intents.rs` | Summarizes one block's effects into `DirectEffectFacts`. |
| `read_target` | `checks/operators.rs` (`??` coalesce check) | Scope-free test of whether an LHS resolves to a saved/store-index place. |

## Notes on code reality

- `function_ref_writes_saved_data` (`writes.rs`) reads each callee's precomputed `direct_effects.saved_writes`, so direct effects must be refreshed *before* the presence walk relies on them.
- `read_target` resolves with `NameScope::default()`, so a single-segment LHS name is keyed as `name:` rather than a binding id. Fine for its boolean is-saved use, but its `ReadTarget` is not comparable for narrowing identity against the scope-aware ones from the walk.
- `FutureEphemeralRootEffects` is a field of `DirectEffectFacts` that no pass ever populates — it always reads empty (only `evolution/intents.rs` reads it). Likely dead pending a future feature.
- `keys.rs` claims `expression_key` is the sole canonical-form owner, yet `binding_key` in the same file independently formats the identical `binding:{id}:{name}` string — a second, consistent producer of that text.

## Tests

`crates/marrow-check/tests` (the `catalog_presence_*` and `discharge_*` files) drive real `.mw` fixtures through `check_project` and assert `presence_proofs()` source/status/place and presence/absence of `CHECK_BARE_MAYBE_PRESENT_READ`. `catalog_presence_narrowing.rs` checks that `if exists` narrows and that a mutation expiring it re-raises the bare read; `discharge_store_key.rs` and `discharge_required_leaf_presence.rs` cover store-index and required-slot discharge.

## Read next

- `presence/walk.rs` — `collect_expr` / `collect_call_expr` / `collect_guarded_block`: how `ReadContext` is chosen and how branches clone and rejoin the narrowed set.
- `presence/effects.rs` — `condition_effects_after_mutations` / `written_target_invalidates`: what `if exists` narrows and how a write expires it (the soundness core).
- `presence/keys.rs` — `expression_key`: the canonical-key format behind all narrowing identity and binding invalidation.
- `presence/proofs.rs` — `read_proof` / `record_read`: context to proof source/status, and the sole bare-maybe-present diagnostic site.
