# Runtime implementation

`marrow-run` interprets checked runtime bodies, mediates host effects, plans
managed writes, and coordinates current project sessions. It is a tree-walking
interpreter, not a bytecode VM.

## Execution

`entry.rs`, `call.rs`, `exec.rs`, `statement.rs`, and `expr.rs` form the
interpreter spine. Runtime values are owned by `value.rs`. Calls execute checked
function bodies; the runtime does not resolve source names or type expressions.

## Durable access

`durable_read.rs`, `read.rs`, `saved_iter/`, and `path.rs` translate checked
places into typed store operations. `write_plan.rs`, `write_dispatch/`,
`group_write.rs`, and `index_maintenance.rs` build managed changes before the
store transaction commits them. Whole assignments, required-field validation,
index maintenance, and transaction rollback converge through this path.

`transaction.rs` owns language transaction nesting and failure behavior.
`host.rs` and `host_effects.rs` mediate clock, environment, filesystem, logging,
context, and entropy access.

## Standard library

The current standard library is split among descriptor tables in
`marrow-schema` and Rust implementations under `std_*` and `stdlib/`. Pure and
host-capable operations share one checked call model. This arrangement is
current, but the intended portable library should move ordinary behavior into
Marrow source above a small intrinsic boundary.

## Sessions

`project_session.rs` currently combines project checking, store open/recovery,
catalog baselining and auto-apply, entry execution, and legacy surface sessions.
This is a broad orchestration owner rather than a narrow runtime boundary; a
change to one lifecycle phase can therefore cross otherwise separate concerns.
