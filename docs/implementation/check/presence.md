# Presence and effect analysis

Optionality is a **type**. A maybe-present source — a sparse field read, a
positional/keyed/unique-index read, a `next`/`prev` neighbor, a `T?`-returning
call, or a standard-library op that may have no result — infers to
`MarrowType::Optional`, and the bare `absent` literal infers to
`MarrowType::Absent`. The **one rule** (`check.unresolved_optional`) is a
type-compatibility check raised at every typed slot site when an optional value is
used where a non-optional `T` is required. There is no separate presence pass and
no parallel presence flag beside any type, call, or ABI fact: a read or call is
maybe-present exactly when its inferred type is `Optional`.

`infer::wrap_maybe_present` is the single site read optionality enters the type
lattice: it wraps a value-position maybe-present read in `MarrowType::optional`
unless flow narrowing has already proven that very place present. The four
eliminators (`?? default`, `?.`, `if const`, `exists`) consume an optional and
produce the resolved type; the one rule fires anywhere else an optional reaches a
`T` slot.

The remaining post-lowering pass (`check_next_id_collisions`) owns only the
`nextId` collision check — the one structural fact the type pass cannot see — plus
the body-local effect summary (`DirectEffectFacts`) that other passes consume.

## Flow narrowing — `T?` → `T`

Narrowing is the only flow-sensitive piece. It does not own *what* is optional
(the type does); it refines a re-read of a stable saved place or a local
`var`/`const`/parameter `T?` binding from `Optional(T)` to `T` once a guard proves
presence, and re-imposes `Optional(T)` when the place could have been cleared (a
saved write, or a `var` reassignment). The state lives in the AST type pass
(`checks/statements.rs`) as a sibling of `RequiredFieldAssignments`: a
`presence/flow.rs::Narrowing` value the statement checker enters and exits across
guarded, looped, and caught scopes. `infer` consults `flow::read_is_narrowed` at
each read; a narrowed read drops its `Optional` layer so no downstream slot fires
the one rule.

Constructs that prove presence: `if exists(place)` (and `cond and exists(place)`)
for its then-block, a fall-through-preventing `if not exists(place)` for the
statements that follow, `if const name = place` for the subject place in its
then-block, and a `for` loop's traversal for each iterated entry read in its body.

**Invalidation is conservative — alias-safe and effect-aware.** A saved write
whose canonical key is not provably distinct from a narrowed key drops the
narrowing (two different key expressions are treated as a possible alias). A
reassigned read binding drops a narrowing keyed on it. A call whose effect
footprint may write the field drops every saved narrowing, fail-closed when the
footprint is imprecise. Any place a loop body clears is re-imposed as optional at
the header, so the next iteration and the post-loop read re-trigger the one rule;
a try/catch body's narrowings likewise do not survive a possible throw. To read
each call's footprint, `analysis.rs` lowers runtime bodies before the type pass.

## The big idea

Narrowing identity is by **span-stripped canonical key**, never structural
`CheckedExpr` equality: two textually equal reads carry different spans, so
`keys.rs` owns the canonical read and binding key formats. A narrowing keyed on
binding ids expires when a read binding is reassigned, when an overlapping saved
write occurs (overlap is by key/member prefix in either direction), or when the
branch it lived in joins back. Effect identity always uses stable schema ids
(`SavedPlaceEffect` = `ResourceId` + `ResourceMemberId` path); an unresolvable
path narrows nothing rather than narrowing on a string key.

## Modules

| File | Responsibility |
| --- | --- |
| `presence.rs` | Module root; defines `check_next_id_collisions` (the only surviving post-lowering pass), the `ReadScope` bundle (`old` binding + narrowed set) the inference threads, and re-exports the flow narrowing API and effect helpers. |
| `presence/flow.rs` | The flow-sensitive `Narrowing` state and the `FlowCtx` query scope. Owns enter/exit of guarded, looped, and caught scopes, the conservative invalidation, and `read_is_narrowed` the inference consults. Lowers a source expression to a `CheckedExpr` and resolves its read target through the shared helpers. |
| `presence/nextid.rs` | Source-order walk that warns (`CHECK_NEXT_ID_COLLISION`) when two ids allocated from one store with no record write between them are both written as record keys. A write advances only the cohorts of the stores it actually wrote — `writes::call_written_stores` reads per-store record/index writes from a call's effect closure, never the coarse `stores_written` set a bare `nextId` peek also enters. |
| `presence/direct.rs` | Body-local effect collector producing `DirectEffectFacts` for one block without expanding callee effects, including typed store roots and direct user-function refs. |
| `presence/effects.rs` | Narrowing algebra: the `exists`/`&&` condition narrowings, negated-exists narrowings, the `for` loop traversal narrowing, and the invalidation rules (key-binding, written-target overlap, saved-wipe). |
| `presence/keys.rs` | Sole owner of the canonical span-stripped narrowing key; extracts `SavedPlaceKey` from the checked saved place. |
| `presence/target.rs` | Resolves an expression to a `ReadTarget`/`ReadPlace`. Saved-place identity consumes checked-place effects from `executable/place.rs`; `saved_target_value` reports a maybe-present value only for a fully-keyed place, so a partial-key composite layer is address-only. A bare `T?` local binding resolves to `ReadPlace::Local` keyed on its scope binding id, so `exists`/`if const` accept and narrow it uniformly with a saved read. Transform `old.<member>` resolution delegates the read-member rule to `evolution/transform_reads.rs`. `read_value_resolves_in_type_scope`/`exists_target_in_type_scope` and `local_maybe_present_read` recognize resolvable local-collection and sparse-field reads for the `??`/`exists` predicates, screening keys through `read_only::guard_subexpr_admissible` so an effectful key (`nextId(^s)`, a write, a throw, an opaque user call) stays unguardable. |
| `presence/writes.rs` | Recursive effect closure through direct callee refs, reading each function's precomputed `DirectEffectFacts` and exposing `write_effects_reachable` and the per-store written set. |
| `presence/read_only.rs` | Checks injected read-only expressions against the allowed runtime surface; owns `guard_subexpr_admissible`, the direct-effect screen that keeps writes, allocations, host calls, throws, and user-function calls out of a guard's key/base. |
| `presence/calls.rs` | Typed-call helpers: maybe-present result test (off the `Optional` return type), neighbor read direction, single-arg collection-view unwrap. |
| `presence/scope.rs` | `NameScope`: frame stack mapping names to monotonic binding ids, including the transform `old` binding when resolving a read against the live type scope. |
| `presence/util.rs` | `push_unique`/`extend_unique` dedup helpers for narrowing/binding lists. |

The one rule, the `wrap_maybe_present` site, and the eliminators live in
`infer.rs`, `checks/operators.rs`, and `checks/calls.rs`; narrowing maintenance
lives in `checks/statements.rs`. The persisted effect forms — `DirectEffectFacts`,
`EffectClosureFacts`, `EntryFootprintFact`, `SavedPlaceEffect` — live in `facts.rs`.

## Entry points

| Symbol | Caller | Role |
| --- | --- | --- |
| `check_next_id_collisions` | `analysis.rs` (after lowering) | The surviving post-lowering structural check. |
| `Narrowing` / `read_is_narrowed` | `checks/statements.rs`, `infer.rs` | Flow narrowing state and the read-site consult that discharges the one rule for a proven-present re-read. |
| `direct_effects_for_block` | `facts.rs` `refresh_direct_effects`, `evolution/intents.rs` | Summarizes one block's effects into `DirectEffectFacts`. |
| `effect_closure` | `program.rs`, flow narrowing | Expands direct callee refs into a transitive summary and the write-reachability bit. |
| `read_value_resolves_in_type_scope` / `exists_target_in_type_scope` | `infer.rs`, `checks/calls.rs` | Classify a maybe-present value read for the wrap site and the `exists` boundary. |
