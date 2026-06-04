# Lane 11: Rust De-Slopification And Hardening

Status: final hardening lane. It starts from fresh scans on current `main`; old
line numbers, chat memory, and completed lane notes are not evidence.

## Contract

Lane 11 proves that semantic lanes removed their prototype paths and weak Rust
structure. It is not a parking lot for unresolved semantic work.

Lane 11 may edit only when ownership is settled and the change is focused. If a
finding belongs to an active semantic owner, Lane 11 returns it to that owner
instead of inventing a compatibility story.

## Required Absence Matrix

Before hardening edits, refresh scans and assign every valid hit one verdict:

- keep production;
- debug/admin only;
- reserved or rejected;
- future-only;
- Scott-pending;
- semantic-owner blocker;
- Lane 11 deletion or split.

The matrix must cover language, runtime, storage, evolution, CLI, LSP, data
tools, serve, backup, restore, docs, tests, and future docs.

## Scan Seeds

Fresh scans should look for:

- `unsafe`;
- crate-root glob preludes and production `use super::*`;
- clippy allowances that hide broad functions or weak structure;
- executable recovery/unknown facts, fallback lookup, sentinel identity strings,
  and duplicate semantic classifiers;
- raw saved paths, backend bytes, raw archive replay, and raw production
  protocol leakage;
- rejected or reserved surfaces such as `@id`, resource-owned identity aliases,
  `merge`, `lock`, saved `inout`, `edit`, patch/update DSLs,
  execution-strategy wording, source-diff identity, and migration scripts;
- `unknown` used as `any`;
- restore that imports orphaned managed cells;
- accidental production serve APIs;
- activation state that becomes migration history instead of job evidence;
- unbounded durable materialization where a bounded page, cursor, or count/probe
  is the contract;
- oversized semantic kernels, duplicate helper passes, catch-all test suites,
  and comments that narrate old edits or obvious control flow.

These patterns are scan seeds, not authority. Each match must be read and
classified against current docs and code.

## Edit Rules

When Lane 11 edits:

- split or delete the touched production path in the same focused change;
- delete legacy branches kept only for old tests, fixtures, compile callers, or
  green-bar pressure;
- migrate tests to source-driven production fixtures when the touched behavior
  changes;
- avoid broad cleanup commits, new compatibility glue, and generic helper
  abstractions;
- preserve only comments that explain durable invariants or soundness
  rationale.

## Verification

Completion requires a clean status, exact base/head, changed-file list, focused
and full gate output, reviewer verdicts, updated lane status, and fresh
absence/sibling scans proving rejected production paths are gone across the
owned area.
