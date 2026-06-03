# Lane 8: Runtime Checked Execution And Write Planner

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.

Goal: production runtime execution consumes checked executable facts, checked
durable places, explicit write plans, transaction state, and index-maintenance
facts. Runtime must not execute source syntax bodies, split source names or saved
paths to recover semantics, or preserve prototype runtime behavior for old tests.

Status: complete for Lane 8 runtime checked execution as of June 3, 2026.
Soundness review, idiom/spec review, formatter, full tests, strict clippy, and
`git diff --check` passed on the lane branch. The Lane 10 blockers below remain
owned by Lane 10 and are not Lane 8 runtime blockers.

## Runtime Contract

- Runtime entry accepts checked entry calls and checked executable bodies.
- Saved reads, writes, deletes, loop traversal guards, and index maintenance use
  checked durable-place facts and catalog identities.
- Durable saved loops stream typed traversal rows through the loop body; local
  in-memory arrays/maps may still materialize as ordinary values.
- Saved durable collections do not materialize as runtime values through
  `keys`, `values`, `entries`, `reversed`, or generic loop fallbacks. Iterate
  them directly.
- `count` and `exists` over saved roots and index branches use bounded count or
  presence probes, not hidden value materialization.
- Write plans own transaction rollback, required-field validation, root clearing,
  index maintenance, and traversal-guard checks.
- `lock`, `merge`, saved `inout`, and production `~` roots are not v0.1 runtime
  features.
- Debug/admin-only tooling may inspect stored bytes, but normal runtime behavior
  is checked-fact driven.

## Area Cleanup

Lane 8 owns runtime cleanup in `crates/marrow-run/**` and runtime-facing tests.
Before completion, review must confirm:

- no production execution of syntax `Block`, `Statement`, or `Expression`;
- no runtime-local saved-path, schema, function-name, enum-member, or store-id
  classifier that duplicates checked facts;
- no compatibility branch, mode flag, or fallback dispatch kept only for old
  behavior;
- saved traversal code is split by root, index, unique-index, and child-layer
  invariants;
- no low-value comments narrate branch behavior or migration history.

## Lane 10 Blockers

Lane 10 owns production cleanup for raw/path-addressed tooling and serve
protocol surfaces. The concrete owning files are `crates/marrow/src/cmd_data.rs`,
`crates/marrow/src/cmd_explain.rs`, `crates/marrow/src/serve/**`,
`crates/marrow/tests/*data*.rs`, `crates/marrow/tests/*explain*.rs`,
`crates/marrow/tests/*serve*.rs`, `docs/data-tools.md`, `docs/serve-protocol.md`,
`docs/cli.md`, and `docs/backend-contract.md`.

Known Lane 10 work:

- `marrow data get`, `marrow data dump`, and `marrow explain ^path` are
  diagnostic/admin inspection surfaces until Lane 10 replaces raw/path-addressed
  production previews with a typed, bounded protocol.
- The old raw `marrow serve saved_children` surface is gone. Current serve
  inspection operations use the `debug_data_*` namespace; Lane 10 owns any
  production child-listing, paging, or preview protocol.
- `marrow serve debug_data_walk` is bounded and session-scoped, but it remains a
  debug/admin inspection operation until Lane 10 defines production
  snapshot/catalog-epoch protocol semantics.
- Backup/restore must use a typed manifest and must not rely on raw path/value
  dumps or serve protocol bytes.

## Tests And Gates

Lane 8 completion requires focused runtime checks, CLI boundary checks that
exercise the runtime, the full workspace test suite, strict clippy, formatter
check, and `git diff --check` with an explicit isolated `CARGO_TARGET_DIR` on
every cargo command.

Required review lenses:

- Soundness review attacks transaction branches, future loop element mutation,
  index updates, optional fields, host effects, stale proof facts, saved
  traversal/materialization, and any path that executes syntax.
- Idiom/spec review checks runtime consumes facts, write planners stay focused,
  compatibility code is deleted, and Rust modules have clear invariants. It also
  rejects oversized runtime/protocol dispatchers, duplicate path classifiers,
  syntax-execution glue, comment sediment, and lane-local cleanup deferred to
  Lane 11.
