# Presence and effect analysis

A post-typecheck static pass over the lowered runtime IR that proves, before runtime, that every read of maybe-present saved data is justified. It walks each function body, constant body, and lowered `evolve transform` body flow-sensitively, tracking *narrowings* from read-site constructs such as `if exists(...)`, `if const`, early-return `if not exists(...)`, loop traversal, coalesce, and unique store-index lookup. The pass emits one `PresenceProofFact` per saved read and raises `CHECK_BARE_MAYBE_PRESENT_READ` when a maybe-present read is reached without a read-site proof. A second body-local entry summarizes each block's saved reads/writes, touched stores/indexes, host capabilities, throws, transactions, and direct user-function callees into `DirectEffectFacts`.

Maybe-present call results use the same typed resolution-site check as saved reads: a `CheckedStdCall` or `CheckedFunctionRef` with `ReturnPresence::MaybePresent` is bare unless it is consumed by `??`, `if const`, `exists`, or returned from another maybe-returning function. User-function calls are not converted into persisted `ReadTarget`s or saved proof places, so a proof for one call expression never narrows a later repeated call.

The pass runs near the end of `analyze_source_project` (`analysis.rs`, after lowering runtime bodies and the evolution transform-effects check). It mutates `program.facts` and pushes diagnostics; it owns no store access of its own.

## The big idea

Narrowing identity is by **span-stripped canonical key**, never by structural `CheckedExpr` equality. Two textually equal reads carry different spans, so `keys.rs` owns the canonical read and binding key formats used for narrowing and invalidation. A narrowing keyed on binding ids expires when any read binding is reassigned, when an overlapping saved write occurs (overlap is by key/member prefix in either direction), or when a branch it lived in joins back. Branch narrowings work on a cloned narrowed set and never escape to the join point. Effect identity always uses stable schema ids (`SavedPlaceEffect` = `ResourceId` + `ResourceMemberId` path); an unresolvable path yields no proof rather than a string-keyed one.

## Parts

- **Flow driver** (`presence/walk.rs`): threads the narrowed set and
  `NameScope`, classifies each read's `ReadContext`, dispatches builtins,
  records proofs.
- **Narrowing algebra** (`effects.rs`): what `exists`/`&&` and loop traversals narrow, and the invalidation rules that expire narrowings.
- **Canonical key** (`keys.rs`): the one owner of the span-free key format.
- **Read resolution** (`target.rs`): expression to `ReadTarget`/`ReadPlace`, and then to a persisted `PresenceProofPlace`. Type-check-only callers use boolean predicates from the same resolver rather than comparable proof targets.
- **Proofs** (`proofs.rs`): the only place the bare-maybe-present diagnostic is raised; maps context to proof source/status and records the fact.
- **Direct effects** (`direct.rs`) and **effect closure** (`writes.rs`, cycle-guarded across user functions).

## Modules

| File | Responsibility |
| --- | --- |
| `presence.rs` | Module root; defines `check_presence` as a thin wrapper over `walk::check_presence` and re-exports `direct_effects_for_block` plus type-check read-resolution predicates. |
| `presence/walk.rs` | Flow-sensitive driver over function, constant, and transform bodies; threads narrowed set + scope, classifies reads, dispatches builtins, invalidates on writes and branch joins. |
| `presence/direct.rs` | Body-local effect collector producing `DirectEffectFacts` for one block without expanding callee effects, including typed store roots and direct user-function refs. |
| `presence/effects.rs` | Narrowing algebra: condition/loop narrowings and the invalidation (key-binding, written-target overlap, removed-on-branch, saved-wipe) rules. |
| `presence/keys.rs` | Sole owner of the canonical span-stripped narrowing key; extracts `SavedPlaceKey` from the checked saved place. |
| `presence/target.rs` | Resolves an expression to a `ReadTarget`/`ReadPlace` and maps it to a persisted `PresenceProofPlace`. Saved-place proof identity consumes checked-place effects from `executable/place.rs`; transform `old.<member>` resolution delegates the top-level read-member rule to `evolution/transform_reads.rs`. |
| `presence/writes.rs` | Recursive effect closure through direct callee refs, reading each function's precomputed `DirectEffectFacts` and exposing `write_effects_reachable`. |
| `presence/proofs.rs` | Builds a `ReadProof`, assigns source/status, records the fact, emits the bare-maybe-present diagnostic. |
| `presence/read_only.rs` | Checks injected read-only expressions against the allowed runtime surface. |
| `presence/calls.rs` | Typed-call helpers: std Path-argument mask, neighbor read direction, single-arg collection-view unwrap. |
| `presence/scope.rs` | `NameScope`: frame stack mapping names to monotonic binding ids, including the transform `old` binding when walking lowered transform bodies and the current function's return presence for maybe-return propagation. |
| `presence/util.rs` | `push_unique`/`extend_unique` dedup helpers for narrowing/binding lists. |

Key types live mostly in `presence/target.rs` (`ReadTarget`, `ReadPlace`), `presence/keys.rs` (`ExprKey`, `SavedPlaceKey`), and `presence/proofs.rs` (`ReadContext`, `ReadProof`). The persisted forms — `DirectEffectFacts`, `EffectClosureFacts`, `EntryFootprintFact`, `PresenceProofFact`/`PresenceProofDraft`, `SavedPlaceEffect` — live in `facts.rs`.

## Entry points

| Symbol | Caller | Role |
| --- | --- | --- |
| `check_presence` | `analysis.rs` (after lowering) | Runs the flow-sensitive walk, mutating facts and pushing diagnostics. |
| `direct_effects_for_block` | `facts.rs` `refresh_direct_effects`, `evolution/intents.rs` | Summarizes one block's effects into `DirectEffectFacts`. |
| `effect_closure` | `program.rs`, presence narrowing | Expands direct callee refs into a unified transitive summary and the write-reachability bit. |
| `read_resolves_in_type_scope` | `checks/operators.rs` | Boolean test for type-checking `??` resolution. It rebuilds enough scope for name shadowing but does not expose a comparable `ReadTarget`. |
| `bindable_saved_value_read_in_type_scope` | `checks/statements.rs` | Boolean test for type-checking `if const`; requires a bindable saved value read. |
| `exists_target_in_type_scope` | `checks/calls.rs` | Boolean test for type-checking `exists(...)`; accepts direct saved read targets and typed maybe-present call targets, so neighbor values remain rejected. |

## Tests

`crates/marrow-check/tests` (the `catalog_presence_*`, `discharge_*`, and
project statement files) drive real `.mw` fixtures through `check_project` and
assert `presence_proofs()` source/status/place and presence/absence of
`CHECK_BARE_MAYBE_PRESENT_READ`.
`crates/marrow-check/tests/cases/catalog_presence_narrowing.rs` checks that
`if exists`, `if const`, and early-return `if not exists` narrow only when their
control flow is sound, and that mutations expire narrowings. The discharge tests
cover store-index, traversal, coalesce, and the absence of declaration-only
required-field proofs.

## Read next

- `presence/walk.rs` — `collect_expr` / `collect_call_expr` / `collect_guarded_block`: how `ReadContext` is chosen and how branches clone and rejoin the narrowed set.
- `presence/effects.rs` — `condition_narrowings` / `written_target_invalidates`: what `if exists` narrows and how a write expires it (the soundness core).
- `presence/keys.rs` — `expression_key`: the canonical-key format behind all narrowing identity and binding invalidation.
- `presence/proofs.rs` — `read_proof` / `record_read`: context to proof source/status, and the sole bare-maybe-present diagnostic site.
