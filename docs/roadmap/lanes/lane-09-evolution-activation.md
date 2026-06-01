# Lane 9: Source-Native Evolution And Activation

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> This lane is one proof-discharge pipeline with multiple command surfaces.

Goal: implement source-native evolution, catalog preview/accept, data-attached
checks, exact witnesses, activation gates, and compatibility windows without
falling back to migration scripts or source-diff identity inference.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-09-evolution-activation`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-09-evolution-activation`

Status: read-only witness matrix design may start now; tracked edits wait for
catalog, presence ledger, tree-cell store, and runtime facts.

## Parallel Safety

This lane may run read-only design review and fixture planning while earlier
lanes build the facts it consumes. Before its dependencies land, record findings
outside tracked files or return them to the orchestrator; do not edit
`docs/data-evolution.md`, `docs/cli.md`, `docs/project-config.md`, tests, or
source. Do not edit checker presence facts, catalog identity, runtime write
planning, or store key shape in parallel with their owning lanes.

Own these files during the code pass:

- evolution modules under `crates/marrow-check/src/` or `crates/marrow-run/src/`
  chosen by boundary review
- `crates/marrow/src/main.rs`
- CLI modules for `catalog` and `evolve` commands
- `crates/marrow/tests/*evolve*.rs`
- `docs/data-evolution.md`
- `docs/cli.md`
- `docs/project-config.md`

## Production Contract

- `marrow check`, data-attached check, `catalog preview`, `catalog accept`,
  `evolve preview`, `evolve apply`, and repair consume one shared
  proof-discharge pipeline.
- Preview is read-only and produces an exact witness.
- Apply consumes only the exact witness and aborts on source, catalog, snapshot,
  engine, affected-ID, or count drift.
- V0.1 compatibility lenses are limited to rename/default compatibility and
  defaulting a newly required field.
- Catalog/runtime metadata declares compatibility windows explicitly.
- Old and new binaries activate only inside those windows; stale writers fail
  closed.

## Prototype Removal Ledger

Replacement behavior: source-native proofs and exact witnesses authorize
catalog/data changes.

Delete or reject:

- migration-script framing as the primary workflow;
- silent source-diff identity preservation;
- transform shims standing in for checked transforms;
- apply paths that do not consume the exact preview witness;
- repair paths that bypass catalog/proof-ledger activation.

Temporary bridge allowed: none for destructive apply. Any transform not
implemented as a checked transform is rejected as transform-required.

## TDD Start

Write failing production-pipeline checks:

- optional field add needs no rewrite;
- required field with default reads old data;
- rename requires source-native intent;
- destructive approval missing, present, and drift cases;
- online index build is invisible to production queries before verify;
- failed index build cannot publish partial index data;
- split/merge transform is rejected as transform-required unless the lane
  implements the checked transform;
- failed apply resumes or rolls back;
- old-binary, new-binary, expired-window, and stale-writer fixtures enforce
  compatibility windows.

Focused command:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-09-evolution-activation \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-09-evolution-activation/Cargo.toml \
    -p marrow --test evolve_cli
```

## Review Lenses

Soundness review attacks witness drift, destructive approval scope, branch
rollback, stale engine metadata, backfill idempotence, compatibility window
expiry, and repair bypasses.

Idiom/spec review checks user-facing terms are `rename`, `default`, `prove`,
`transform`, `retire`, `rebuild`, and `repair`, and that no migration-script
product story leaks back into canonical docs.

## Integration Gate

Run the full central gate. Add scans:

```sh
rg -n 'migration script|source diff|best effort|auto.*rename|shim|transform' \
    /Users/scottwilliams/Dev/marrow-lane-09-evolution-activation/crates \
    /Users/scottwilliams/Dev/marrow-lane-09-evolution-activation/docs
```

Every match must describe rejection, future-only scope, or the checked
source-native workflow.

## Starter Prompt

Continue Marrow v0.1 Lane 9 in `/Users/scottwilliams/Dev/marrow-lane-09-evolution-activation`.
Use branch `lane-09-evolution-activation`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-09-evolution-activation`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
Do not start production code until catalog identity, the presence ledger,
tree-cell store contracts, and runtime checked facts are integrated. Implement
one proof-discharge pipeline for check/catalog/evolve/repair surfaces, exact
witness preview/apply, compatibility windows, and rejection of migration-script
or transform shims. Before those dependencies land, do read-only design only and
make no tracked edits. Leave the worktree dirty for soundness and idiom/spec
review.
