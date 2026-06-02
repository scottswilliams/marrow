# Lane 8: Runtime Checked Execution And Write Planner

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> This is the lane that deletes production syntax-body execution.

Goal: make production runtime execution consume checked facts or checked IR,
with explicit durable places, effects, write plans, transaction behavior, and
index maintenance.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-08-runtime-checked`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-08-runtime-checked`

Status: inventory and design may start now; production code waits for Lane 5,
Lane 6, and the Lane 7 tree-cell address API.

## Parallel Safety

This lane may run read-only runtime inventory in parallel with earlier lanes.
Do not edit production runtime code until store facts, durable places, the
presence ledger, and the tree-cell address API are available. Do not split
runtime files across competing orchestrators; `marrow-run` is one vertical
replacement.

Own these files during the code pass:

- `crates/marrow-run/src/*.rs`
- focused runtime fixtures under `crates/marrow-run/tests/`
- runtime-facing checked facts under `crates/marrow-check/src/` when required
- `crates/marrow/tests/run_cli.rs`
- `docs/language/control-flow-and-effects.md`
- `docs/language/resources-and-storage.md`

Do not change parser syntax, catalog acceptance workflow, tree-cell physical
keys, or evolution apply semantics in this lane.

## Area Cleanup Gate

This lane owns the complete cleanup of the runtime execution area across
checked runtime entry, durable-place reads, write planning, transactions, host
effects, runtime tests, and runtime-facing docs. It must delete syntax-body
execution and runtime-local path/schema classifiers in its area instead of
leaving a second runtime model for a later lane.

Before handing the lane to review:

- split checked execution, durable-place reads, write planning, transactions,
  index maintenance, and host-effect handling by invariant;
- migrate runtime tests, fixtures, and callers to checked facts or checked IR
  instead of keeping the syntax interpreter, raw `FunctionDecl` entrypoints, or
  fallback dispatch so old tests keep passing;
- delete production syntax execution paths instead of wrapping them in mode
  flags or compatibility helpers;
- delete dead `lock`, `merge`, saved `inout`, string/path classifier, and raw
  syntax execution helpers introduced or exposed by this lane;
- delete comments that narrate statement branches, migration state, or why a
  large dispatcher is safe;
- preserve only comments that explain durable write, rollback, or host-effect
  soundness constraints;
- ensure the idiom/spec reviewer explicitly checks touched Rust for oversized
  runtime functions, duplicate path classifiers, syntax-execution glue, comment
  sediment, and lane-local cleanup deferred to Lane 11.

## Production Contract

- Runtime entry accepts checked executable facts or IR, not syntax bodies.
- Saved reads and writes use checked durable places.
- Runtime consumes the presence ledger rather than recomputing read totality.
- The checked-effect model retains a named future slot for ADR 0209 ephemeral
  reads and writes, but runtime exposes no production `~` root behavior.
- Assignments, `edit`, `delete`, and assertions lower to explicit write plans.
- Root assignment exposes subtree clearing effects.
- Field/path assignment and `edit` preserve omitted data and update indexes.
- Irreversible host effects are forbidden inside rollback-sensitive
  transactions.
- Runtime preserves tree, sequence, and keyed-layer shapes; no flat list model
  becomes the production collection contract.
- `lock`, `merge`, and saved `inout` are not production runtime features.
- Principal/request-context effects stay future-reserved.

## Prototype Removal Ledger

Replacement behavior: checked facts/IR fully determine what runtime executes.

Delete or isolate:

- production execution of syntax `Block`, `Statement`, or `Expression`;
- temporary syntax-body bridge from the checked-model migration;
- runtime splitting of `::`, saved paths, function names, enum members, or
  store identities;
- saved `inout` writeback;
- runtime schema/path classifiers that duplicate checker facts;
- hidden merge or lock semantics.

Production bridge: none for execution. Debug interpreters must be named
debug/admin surfaces and excluded from `run` and normal CLI paths.

## TDD Start

Write failing checks first:

- architecture test proving production runtime cannot execute raw syntax;
- exact root assignment reports subtree clearing;
- field assignment and `edit` preserve omitted data;
- delete and existence assertions lower to write plans;
- transactions roll back nested failures and host effects correctly;
- optional/default reads use checked proof facts;
- missing required production data fails activation or run;
- index maintenance covers unique duplicate rollback and absent-component
  removal;
- typed references read/write without implicit joins, cascade delete, or
  existence checks;
- `lock`, `merge`, and saved `inout` stay rejected.
- accidental `cache ~`, `ensure ~`, `Id(~...)`, or production `~` root behavior
  is absent from runtime execution.

Focused commands:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-08-runtime-checked \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-08-runtime-checked/Cargo.toml \
    -p marrow-run

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-08-runtime-checked \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-08-runtime-checked/Cargo.toml \
    -p marrow --test run_cli
```

## Review Lenses

Soundness review attacks transaction branches, future loop element mutation,
index updates, optional fields, host effects, stale proof facts, and any path
that executes syntax.

Idiom/spec review checks runtime consumes facts, write planners stay focused,
compatibility code is deleted, and Rust modules have clear invariants. It also
rejects oversized runtime dispatchers, duplicate path classifiers, syntax
execution glue, comment sediment, and lane-local cleanup deferred to Lane 11.

## Integration Gate

Run the full central gate. Add syntax-runtime absence scans:

```sh
rg -n 'Block|Statement|Expression|split\\(\"::\"\\)|inout|merge|lock|cache\s*~|ensure\s*~|Id\s*\(\s*~' \
    /Users/scottwilliams/Dev/marrow-lane-08-runtime-checked/crates/marrow-run/src
```

Every match must be a deleted-path test, debug/admin-only path, or non-runtime
type name with no production execution role.

## Starter Prompt

Continue Marrow v0.1 Lane 8 in `/Users/scottwilliams/Dev/marrow-lane-08-runtime-checked`.
Use branch `lane-08-runtime-checked`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-08-runtime-checked`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
Do read-only inventory/design now if needed, but do not start production runtime
edits until Lane 5 store facts, Lane 6 catalog/presence/enum facts, and the Lane
7 tree-cell address API are on `main`. Replace AST-body execution with checked
facts or checked IR, implement explicit write plans and transaction behavior,
delete runtime string/path classifiers, and prove ADR 0209 `~` roots have no
production runtime behavior beyond a named future checked-effect slot. No
legacy survival for green tests: migrate runtime tests, fixtures, and callers to
checked facts or checked IR instead of keeping the syntax interpreter, raw
`FunctionDecl` entrypoints, fallback dispatch, `lock`, `merge`, or saved
`inout`. Before review, satisfy the Area Cleanup Gate: split checked execution,
durable-place reads, write planning, transactions, index maintenance, and
host-effect handling; delete syntax-body execution and runtime path/schema
classifiers. Leave the worktree dirty for soundness and idiom/spec review.
