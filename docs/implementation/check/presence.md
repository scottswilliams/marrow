# Presence and effect analysis

A post-typecheck static pass over the lowered runtime IR that proves, before runtime, that every read of maybe-present saved data is justified. It walks each function body, constant body, and lowered `evolve transform` body flow-sensitively, tracking *narrowings* from read-site constructs such as `if exists(...)`, `if const`, early-return `if not exists(...)`, loop traversal, coalesce, and unique store-index lookup. The pass emits one `PresenceProofFact` per saved read and raises `CHECK_BARE_MAYBE_PRESENT_READ` when a maybe-present read is reached without a read-site proof. A second body-local entry summarizes each block's saved reads/writes, touched stores/indexes, host capabilities, throws, transactions, and direct user-function callees into `DirectEffectFacts`.

Maybe-present call results use the same typed resolution-site check as saved reads: a `CheckedStdCall` or `CheckedFunctionRef` with `ReturnPresence::MaybePresent` is bare unless it is consumed by `??`, `if const`, `exists`, or returned from another maybe-returning function. User-function calls are not converted into persisted `ReadTarget`s or saved proof places, so a proof for one call expression never narrows a later repeated call.

Two read shapes are resolvable maybe-present reads that carry no persisted proof: a local-collection indexed read of a bound name (`xs(pos)`, `counts(k)`) and a sparse-field read of a *bound* materialized resource value. That base must be a bound name (`book.subtitle`, a caught `err.help`, a loop-bound group entry) or a chained unkeyed group layer rooted at one (`person.address.zip`) — never a call or constructor in the read place. A call or constructor result is guardable only after it is bound to a name (`const b = makeBook()` then `b.subtitle ?? d`), because evaluating an inline call as the guard base would run its body, which may write saved data, open a transaction, call a host capability, or throw. `target.rs::local_maybe_present_read` recognizes the bound shapes for the type-check predicates, and `walk.rs` raises the bare diagnostic for an unguarded one. The runtime resolves these at the read site by catching the absent fault, so the checker records no saved-data proof. The guardable set is widened strictly by construction. The base is a bound name with no call in the read place, so it carries no effect; a local-collection read's key sub-expressions are screened through `read_only.rs::guard_subexpr_admissible`. That screen rejects a write, an allocation (`append`/`nextId`), a host call, a throw, or any user-function call — opaque before per-function closures exist — so `exists(append(xs, v))`, `exists(nextId(^s))`, and a guard keyed by `nextId(^s)` or any effectful function all stay rejected.

Compound assignment is a read-modify-write statement. `walk.rs` records its
target as a normal bare read, so maybe-present saved and local collection reads
must already be discharged by narrowing; it then records the right-hand
expression as a normal bare expression and invalidates the written target just
like plain assignment. `direct.rs` still reports both the saved read and saved
write effects for effect summaries.

The pass runs near the end of `analyze_source_project` (`analysis.rs`, after lowering runtime bodies and the evolution transform-effects check). It mutates `program.facts` and pushes diagnostics; it owns no store access of its own.

## The big idea

Narrowing identity is by **span-stripped canonical key**, never by structural `CheckedExpr` equality. Two textually equal reads carry different spans, so `keys.rs` owns the canonical read and binding key formats used for narrowing and invalidation. A narrowing keyed on binding ids expires when any read binding is reassigned, when an overlapping saved write occurs (overlap is by key/member prefix in either direction), or when a branch it lived in joins back. Branch narrowings work on a cloned narrowed set and never escape to the join point. Effect identity always uses stable schema ids (`SavedPlaceEffect` = `ResourceId` + `ResourceMemberId` path); an unresolvable path yields no proof rather than a string-keyed one.

## Parts

- **Flow driver** (`presence/walk.rs`): threads the narrowed set and
  `NameScope`, classifies each read's `ReadContext`, dispatches builtins,
  records proofs.
- **Narrowing algebra** (`effects.rs`): what `exists`/`&&` and loop traversals narrow, and the invalidation rules that expire narrowings.
- **Canonical key** (`keys.rs`): the one owner of the span-free key format.
- **Read resolution** (`target.rs`): expression to `ReadTarget`/`ReadPlace`, and then to a persisted `PresenceProofPlace`. Also recognizes proof-free resolvable local/sparse reads for the type-check predicates. Type-check-only callers use boolean predicates from the same resolver rather than comparable proof targets.
- **Proofs** (`proofs.rs`): for saved reads, maps context to proof source/status, records the fact, and raises the bare-maybe-present diagnostic. `walk.rs` raises the same diagnostic for an unguarded maybe-present call or local/sparse read, which carry no persisted proof.
- **Direct effects** (`direct.rs`) and **effect closure** (`writes.rs`, cycle-guarded across user functions).

## Modules

| File | Responsibility |
| --- | --- |
| `presence.rs` | Module root; defines `check_presence` as a thin wrapper that runs `walk::check_presence` then `nextid::check_next_id_collisions`, and re-exports `direct_effects_for_block` plus type-check read-resolution predicates. |
| `presence/walk.rs` | Flow-sensitive driver over function, constant, and transform bodies; threads narrowed set + scope, classifies reads, dispatches builtins, invalidates on writes and branch joins. |
| `presence/nextid.rs` | Source-order walk over each body that warns (`CHECK_NEXT_ID_COLLISION`) when two ids allocated from one store with no record write between them are both written as record keys. An allocation is a direct `nextId(^store)`, a call to an allocator function (one whose every return originates from `nextId(^store)`, following a returned name back through its *immutable* local initializer so the `const n = nextId(^s); return n` shape counts), or a name bound to such an initializer; a constructed `Id(^store, key)` is not. A returned name is followed only through `const` bindings: a `var` reassigned anywhere in the body is dropped before following, so a helper whose returned value is not unconditionally a fresh allocation does not warn (the dangerous direction for a warning is a false positive on valid code). The conservative residual — a `var` reassigned from a constructed id to `nextId` then returned will not warn — is acceptable for a safety-net warning. A write advances only the cohorts of the stores it actually wrote — `writes::call_written_stores` reads the per-store record (`saved_writes`) and index (`saved_index_writes`) writes from a call's effect closure, never the coarse `stores_written` set that a bare `nextId` peek also enters — so an unmodeled write can suppress a collision only on the store it touched. |
| `presence/direct.rs` | Body-local effect collector producing `DirectEffectFacts` for one block without expanding callee effects, including typed store roots and direct user-function refs. |
| `presence/effects.rs` | Narrowing algebra: condition/loop narrowings, the loop-binding value type a `for` body runs under, and the invalidation (key-binding, written-target overlap, removed-on-branch, saved-wipe) rules. |
| `presence/keys.rs` | Sole owner of the canonical span-stripped narrowing key; extracts `SavedPlaceKey` from the checked saved place. |
| `presence/target.rs` | Resolves an expression to a `ReadTarget`/`ReadPlace` and maps it to a persisted `PresenceProofPlace`. Saved-place proof identity consumes checked-place effects from `executable/place.rs`; `saved_target_value` reports a maybe-present value only for a fully-keyed place (`executable/place::place_fully_keyed`), so a partial-key composite layer is address-only, never a value to default. Transform `old.<member>` resolution delegates the top-level read-member rule to `evolution/transform_reads.rs`. The guard-acceptance predicates first screen the read's saved place through `guard_saved_keys_admissible`/`saved_key_args_admissible`, which run every identity, layer, and index key through `read_only::guard_subexpr_admissible`, so an effectful key (`nextId(^s)`, a write, a throw, an opaque user call) makes the read unguardable for a direct saved read and for a `next`/`prev` neighbor seek whose subject resolves through that place. The screen sits in the predicates only: the bare-read diagnostic and write-invalidation still resolve the read, so an unguarded or written effectful-key read is not lost. `local_maybe_present_read` recognizes proof-free resolvable local-collection and sparse-field reads, screening local-collection keys through the same screen, resolving a sparse-field base — a bound name or a chained group layer rooted at one — through `infer::member_value_type`, and keying sparse-field classification on `infer::sparse_member`. A call or constructor in a sparse-field base position is not a bound value, so it is not guardable until its result is bound to a name. |
| `presence/writes.rs` | Recursive effect closure through direct callee refs, reading each function's precomputed `DirectEffectFacts` and exposing `write_effects_reachable`. |
| `presence/proofs.rs` | Builds a `ReadProof`, assigns source/status, records the fact, emits the bare-maybe-present diagnostic. A direct address-only saved read (a partial-key composite layer) is not a maybe-present value, so it records no proof and raises no diagnostic — the `check.layer_not_value` descent owns that mistake. |
| `presence/read_only.rs` | Checks injected read-only expressions against the allowed runtime surface; also owns `guard_subexpr_admissible`, the direct-effect screen that keeps writes, allocations, host calls, throws, and user-function calls out of a presence guard's key/base. |
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
| `read_resolves_in_type_scope` | `checks/operators.rs` | Boolean test for type-checking `??` resolution; accepts saved reads, `next`/`prev` neighbor values, maybe-present calls, and proof-free local/sparse reads. It rebuilds enough scope for name shadowing but does not expose a comparable `ReadTarget`. |
| `bindable_saved_value_read_in_type_scope` | `checks/statements.rs` | Boolean test for type-checking `if const`; accepts a bindable saved value read, a `next`/`prev` neighbor value, a maybe-present call, or a resolvable local/sparse read. |
| `exists_target_in_type_scope` | `checks/calls.rs` | Boolean test for type-checking `exists(...)`; accepts direct saved read targets, `next`/`prev` neighbor values, typed maybe-present call targets, and resolvable local/sparse reads. A saved or neighbor read whose key argument carries an effect is not a guardable target, so an effectful key stays rejected. |

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
