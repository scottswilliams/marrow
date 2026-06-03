# Lane 8: Runtime Checked Execution And Write Planner

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.

Goal: production runtime execution consumes checked executable facts, checked
durable places, explicit write plans, transaction state, and index-maintenance
facts. Runtime must not execute source syntax bodies, split source names or saved
paths to recover semantics, or preserve prototype runtime behavior for old tests.

Status: active repair. The runtime execution replacement is substantially
landed, but the lane is not complete until the strict gate, soundness review, and
idiom/spec review pass on the integrated branch. Remaining protocol and tooling
surfaces that still expose raw/path-addressed inspection are Lane 10 blockers,
not evidence that Lane 8 is perfect.

## Runtime Contract

- Runtime entry accepts checked entry calls and checked executable bodies.
- Saved reads, writes, deletes, loop traversal guards, and index maintenance use
  checked durable-place facts and catalog identities.
- Durable saved loops stream typed traversal rows through the loop body; local
  in-memory arrays/maps may still materialize as ordinary values.
- Write plans own transaction rollback, required-field validation, root clearing,
  index maintenance, and traversal-guard checks.
- `lock`, `merge`, saved `inout`, raw syntax-body execution, and production `~`
  roots are not v0.1 runtime features.
- Debug/admin-only tooling may inspect stored bytes, but normal runtime behavior
  is checked-fact driven.

## Cleanup Gate

Before integration, review the touched runtime area for:

- no production execution of syntax `Block`, `Statement`, or `Expression`;
- no runtime-local saved-path, schema, function-name, enum-member, or store-id
  classifier that duplicates checked facts;
- no compatibility branch, mode flag, or fallback dispatch kept only for old
  behavior;
- no broad dispatcher that should be split by invariant;
- no low-value comments narrating branches or migration history.

## Tests And Gates

Lane 8 must pass focused runtime checks, CLI boundary checks that exercise the
runtime, the full workspace test suite, strict clippy, formatter check, and
`git diff --check` with an explicit isolated `CARGO_TARGET_DIR` on every cargo
command.

## Lane 10 Blockers

Lane 10 owns production cleanup for the data/serve/explain protocol surfaces:

- raw/path-addressed CLI inspection (`data get`, `data dump`, `explain ^path`)
  must remain diagnostic/admin-only until a typed production preview protocol
  lands;
- serve protocol path JSON and cursors must be checked, bounded, and
  snapshot/catalog-epoch scoped;
- backup/restore must use a typed manifest and must not rely on raw path/value
  dumps.
