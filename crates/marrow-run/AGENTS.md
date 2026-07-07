# marrow-run — Agent Notes

A tree-walking interpreter over the checked program: evaluates entries, plans managed writes, reads
and iterates saved data, applies data evolution, and hosts the project surface sessions and stdlib
boundary.

Never re-resolve names or re-parse op strings — the checker stamps every call target, capability, and
conversion kind, so each path branches on a typed kind, not source spelling. Writes are
plan-then-commit: build a full typed `WritePlan`, then commit atomically. Meter and page every read
(the materialization budgets are the model, not a violation). A checker-proven-unreachable branch
returns a typed `RuntimeError` and fails closed, not `panic!`/`unreachable!`. `Value` is the single
owner of scalar conversion to and from store form.

Map: [docs/implementation/runtime/](../../docs/implementation/runtime/README.md).
