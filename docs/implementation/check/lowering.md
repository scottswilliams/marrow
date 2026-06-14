# Lowering

Lowering turns already-checked source into the `Checked*` executable IR that `marrow-run` evaluates. The checker has already resolved names and proven well-typedness; lowering bakes that resolution into typed nodes so the runtime needs no name resolution and no further type lookup. Every call becomes a concrete `CheckedCallTarget`, every durable read/write becomes a precomputed `CheckedSavedPlace`, and every statement/expression becomes a `Checked*` node carrying resolved identity (function/resource/enum/store refs, member ids, catalog ids). User-function call refs carry `ReturnPresence`, and `return absent` lowers to its own statement. It is a one-way bridge from the checker into the runtime form.

Lowering runs after type checking and fills each function's `runtime_body`; `program.rs` enforces the positional function-to-declaration zip described in [types.md](types.md). Lowering is fallible by design: every `lower*` returns `Option`, and a body that fails to resolve stays `None`, which discharge and the runtime treat as not-executable rather than an error.

## Parts

- **Call resolution** — `CheckedCallTarget::for_call` decides what a call means, first-match-wins: saved-path read, local collection, constructor, pure builtin/std op, then user function. Builtins/std only match a pure call shape (no named or moded args).
- **Durable addressing** — `place.rs` precomputes a `CheckedSavedPlace` for each durable read/write: store id, resource, member/index/layer navigation, identity keys, and a `CheckedSavedTerminal` (Record/Field/Index). A place advances only while its terminal is still `Record`; once specialized it is a single unambiguous address.
- **Expressions and statements** — `expr.rs` and `stmt.rs` recursively lower syntax into `CheckedExpr` and `CheckedStmt`/`CheckedBody`, threading a mutable lexical scope for local bindings and inference. `saved_place` precomputation rides on the expr nodes; pure value expressions carry `None`.
- **Runtime value types** — `checked_runtime_value_type` converts the checker's `MarrowType` into `CheckedRuntimeValueType`, resolving enum members, identity key shapes, and sequence/tree nesting.

Identity refs (`CheckedFunctionRef`/`ResourceRef`/`EnumRef`) use positional `ptr::eq` indices into the program snapshot, so the IR is only valid against the exact `CheckedProgram` it was lowered from. `checked_activation_root_places` overlays proposed (uncommitted) catalog ids, so evolution/activation sees the about-to-be-committed identity.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/executable.rs` | Module root: re-exports the `Checked*` IR, defines `CheckedExecutableContext`, the ref enums, and the two public saved-place functions. |
| `crates/marrow-check/src/executable/call_target.rs` | Resolves a call into a concrete `CheckedCallTarget`; maps builtin names and their attached-data/neighbor read traits. |
| `crates/marrow-check/src/executable/expr.rs` | `CheckedExpr` and the saved-place IR; recursive expression lowering; function/enum/member ref construction. |
| `crates/marrow-check/src/executable/place.rs` | Builds `CheckedSavedPlace` through the Record→Field/Index/Layer terminal state machine; overlays proposed catalog ids. |
| `crates/marrow-check/src/executable/runtime_value.rs` | `CheckedRuntimeValueType` and resource-constructor IR; `MarrowType` → runtime value-type conversion. |
| `crates/marrow-check/src/executable/stmt.rs` | `CheckedBody`/`CheckedStmt`; statement lowering via grouped helpers (binding/write, branch, loop, effect, match) with scope threading. |
| `crates/marrow-check/src/executable/syntax_parts.rs` | Leaf parts: args/arg-mode, literals, operators, interpolation, for-bindings, match arms, else-if, catch clauses. |
| `crates/marrow-check/src/executable/walk.rs` | Read-only walks over lowered expressions and statements for downstream facts. |

## Read next

- `CheckedBody::lower` in `executable/stmt.rs` — top-level body lowering; the entry the rest hangs off.
- `CheckedExpr::lower` in `executable/expr.rs` — the recursive heart; how every expression form and its attached place/enum member are built.
- `CheckedCallTarget::for_call` in `executable/call_target.rs` — the ordered call-resolution dispatcher (the most semantically loaded decision in the IR).
- `checked_field_place` / `checked_call_place` in `executable/place.rs` — how durable addresses are precomputed and progressively specialized.
- `CheckedBody::lower` call site in `program.rs` (lower bodies loop, `lower_transform_body`) — the caller that enforces the positional zip and None-on-failure invariants.
