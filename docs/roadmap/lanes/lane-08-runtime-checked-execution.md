# Lane 8: Runtime Checked Execution And Write Planner

Goal: production runtime execution consumes checked executable facts, checked
durable places, explicit write plans, transaction state, and index-maintenance
facts. Runtime must not execute source syntax bodies, split source names or saved
paths to recover semantics, or preserve prototype runtime behavior for old tests.

Status: integrated foundation. Future edits in this area are regressions,
hardening work, or explicit follow-up lanes; this file is a historical contract
reference.

## Runtime Contract

- Runtime entry accepts checked entry calls and checked executable bodies.
- Saved reads, writes, deletes, loop traversal guards, and index maintenance use
  checked durable-place facts and catalog identities.
- Durable saved loops stream typed traversal rows through the loop body; local
  in-memory arrays/maps may still materialize as ordinary values.
- Saved durable collections do not materialize as runtime values through `keys`,
  `values`, `entries`, `reversed`, or generic loop fallbacks. Iterate them
  directly.
- `count` and `exists` over saved roots and index branches use bounded count or
  presence probes, not hidden value materialization.
- Write plans own transaction rollback, required-field validation, root clearing,
  index maintenance, and traversal-guard checks.
- `lock`, `merge`, saved `inout`, and production `~` roots are not v0.1 runtime
  features.
- Debug/admin-only tooling may inspect stored bytes, but normal runtime behavior
  is checked-fact driven.

## Reopen Criteria

Reopen this lane only for a concrete runtime regression: production syntax-body
execution, runtime-local saved-path/schema/function-name/enum/store classifiers,
compatibility branches kept for old behavior, unbounded saved-data
materialization, incorrect transaction rollback, missing index maintenance, or
write paths that bypass checked facts.
