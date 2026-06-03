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

## Completion Claim Discipline

Lane 9 may report **audit complete**, **code ready for review**, **blocked**, or
**lane complete**. It may not report "done" after only designing the witness
matrix, listing blockers, or passing one evolve test. If any dependency is not
landed on `main`, the production-code status is **blocked** and the only valid
deliverable is a read-only audit packet.

Before a lane-complete claim, Lane 9 must prove the whole evolution family is
handled:

- `check`, data-attached check, catalog preview/accept, evolve preview/apply,
  repair admission, and compatibility-window checks consume one proof-discharge
  pipeline;
- the exact preview witness is the only apply input, and every drift dimension
  has a failing fixture before implementation;
- migration scripts, source-diff identity inference, best-effort rename,
  transform shims, repair bypasses, and hidden history ledgers are rejected,
  absent, or explicitly future-only in code, docs, tests, and fixtures;
- a sibling scan covers catalog, evolve, repair, maintenance, compatibility,
  transform, CLI docs, and stale tests after each rejected prototype workflow is
  removed;
- touched Rust is split by invariant before review: preview, witness
  validation, apply, compatibility windows, repair admission, and CLI rendering
  cannot collapse into one broad dispatcher.

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

## Feature-Surface Audit Gate

Lane 9 owns the verdicts for catalog, evolution, activation, repair, and
data-attached workflow surfaces. Before code review, classify each surface as
keep production, debug/admin only, rename/rescope, or delete.

Known evolution/activation suspects:

- Migration scripts, migration DSLs, hidden database migration ledgers, and
  source-diff identity inference: delete as product stories.
- Best-effort rename, automatic identity preservation, and transform shims:
  reject unless represented by source-native intent and checked proof facts.
- Compatibility lenses: keep only the v0.1 limited rename/default cases; no
  general old-schema adapter runtime.
- Transform workflows: keep only checked transform-required facts unless this
  lane implements the checked transform path.
- Repair and maintenance entrypoints: keep only as explicit proof-ledger or
  witness-bound workflows; no bypass of activation, catalog, engine, or data
  checks.
- Catalog/evolve CLI names and docs: keep only commands that match the accepted
  source-native preview/apply model.

If a verdict needs runtime, store, checker identity, or tooling protocol changes
outside this lane's owned files, return a blocker to that owner.

## Area Cleanup Gate

This lane owns the complete cleanup of source-native evolution and activation
across proof discharge, catalog/evolve commands, exact witnesses, compatibility
windows, repair admission, docs, fixtures, and tests. It must delete
migration-script framing, source-diff identity inference, and transform shims in
its area instead of leaving a second evolution model for a later lane.

Before handing the lane to review:

- complete the evolution feature-surface verdicts and turn them into tests,
  docs deletion, docs rewrite, or owning-lane blockers;
- split preview, exact witness validation, apply, compatibility-window checks,
  repair admission, and CLI rendering by invariant;
- migrate or delete tests, fixtures, and callers that depend on migration
  scripts, source-diff identity inference, best-effort transforms, or stale
  activation behavior instead of keeping legacy evolution branches for them;
- keep transform-required rejection in the shared proof-discharge path, not in
  adapter-local fallbacks;
- delete dead migration-script, source-diff identity, best-effort rename, and
  transform-shim helpers introduced or exposed by this lane;
- delete comments that narrate migration history, summarize obvious apply
  branches, or explain temporary compatibility glue;
- preserve only comments for non-obvious witness drift, destructive approval,
  recovery, or stale-writer constraints;
- ensure the idiom/spec reviewer explicitly checks touched Rust for oversized
  evolution functions, duplicate witness/proof classifiers, migration shims,
  comment sediment, and lane-local cleanup deferred to Lane 11.

## Production Contract

- `marrow check`, data-attached check, `catalog preview`, `catalog accept`,
  `evolve preview`, `evolve apply`, and repair consume one shared
  proof-discharge pipeline.
- Preview is read-only and produces an exact witness.
- Apply consumes only the exact witness and aborts on source, catalog, snapshot,
  engine, affected-ID, or count drift.
- V0.1 compatibility lenses are limited to rename/default compatibility and
  defaulting a newly required field.
- Migration scripts, source-diff identity inference, and hidden schema-history
  ledgers are not v0.1 product surfaces.
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

Production bridge: none for destructive apply. Any transform not implemented as
a checked transform is rejected as transform-required.

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
- evolution feature-surface tests prove migration-script, source-diff identity,
  best-effort rename, and unchecked transform workflows are rejected or absent.

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
product story leaks back into canonical docs. It also rejects oversized
evolution/apply dispatchers, duplicate witness/proof classifiers, migration
shims, comment sediment, and lane-local cleanup deferred to Lane 11.

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
First inspect current `main`, worktrees, and dependency status. If Lane 6
catalog/presence facts, Lane 7 tree-cell store contracts, or Lane 8 finalized
runtime checked facts are not landed, stop production edits and return an
**audit complete** or **blocked** packet, not a done claim. That packet must
include the witness matrix, dependency blockers, suspect evolution surfaces,
and the first failing tests you will write once code is unblocked.

When dependencies are landed, implement one proof-discharge pipeline for
check/catalog/evolve/repair surfaces, exact witness preview/apply,
compatibility windows, and rejection of migration-script or transform shims.
Complete the evolution feature-surface audit before code review: catalog,
evolve, activation, compatibility lens, checked transform, repair, maintenance,
and stale CLI/docs workflows must match the source-native preview/apply model or
be deleted/demoted. No legacy survival for green tests: migrate/delete tests,
fixtures, and callers that depend on migration scripts, source-diff identity
inference, best-effort transforms, or stale activation behavior.

Do not stop after fixing one rejected workflow. After each fix, scan the sibling
family: catalog/evolve commands, repair/maintenance paths, compatibility docs,
transform-required fixtures, and tests. Before review, satisfy the Area Cleanup
Gate: split preview, exact witness validation, apply, compatibility-window
checks, repair admission, and CLI rendering; delete migration-script,
source-diff identity, best-effort rename, and transform-shim helpers. Leave the
worktree dirty for soundness and idiom/spec review. A final done claim must
include the completion evidence packet required by the central plan.
