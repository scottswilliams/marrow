# Wave 2 — Remaining Policy Fixes (continuation plan)

Status as of 2026-06-11. Wave 1 (engine-resident-catalog), the highest-severity
Wave 2 behavioral fixes, and the remaining owner-approved Wave 2 policy fixes are
shipped to `main`. This file is the closeout ledger for the probe findings they map to.

Repros: `.build/marrow-hardening/PROBE-FINDINGS-2026-06-09.md` (findings B38–B75).
Decisions: `.build/marrow-hardening/POLICY-DECISIONS-2026-06-09.md` and the project
memory note `marrow-policy-decisions-accepted`. Build each remaining fix the same way
the shipped ones were: own worktree off `main`, own `CARGO_TARGET_DIR`, TDD with the
repro as oracle, soundness + idiom review, full gate, then fast-forward integrate.

## Shipped (do not redo)

- Wave 1 — engine-resident catalog refactor (6 commits, `main` `…5f53709`).
- DIAG-7 (B38/B39) — depth limits: parser nesting limit `check.nesting_limit` 256,
  runtime `run.recursion_limit` 1024, 256 MiB worker stack on the dispatch chokepoint.
- EVO-1/2/3 (B43/B44/B52/B53) — fence a populated destructive drop of a member or whole
  resource/store at preview/apply/run; index drop auto-discharges; bare rename fences.
- DUR-1/3/4 (B54/B58/B60) — store open is fail-closed: `catch_unwind` backstop →
  `store.corruption`, damage-faithful redb mapping, typed `store.recovery_required`.
- CAT-1/3 (B42/B47/B50/B57) — an uncommitted/pending catalog id is "no saved data", not
  `store.corruption`, across data tools and serve. Resolves the B-DATA-ROOTS backlog item.
- FEAT-1 (B71) — iterate a date/duration/instant-keyed layer (`saved_key_to_value` total).
- DUR-5 (manifest folded into the backup archive checksum) — landed in Wave 1 Lane 6.
- B59 — verified closed by DUR-5: injected `commit_id` metadata is part of the
  folded manifest checksum and restore rejects it as `restore.corrupt_chunk`.
- DIAG-1/2/3 (B66/B69/B72) — reject unsupported string escapes at check, reject unknown
  ops in closed pure std modules, and enforce dotted lowercase `ErrorCode` text through
  the shared checker/runtime grammar.
- CLI consistency B73/B75 — `lsp` extra args and duplicate `--entry`/`--port` are usage
  errors; duplicate single-valued flags no longer silently take the last value.
- CLI consistency B68 — interpolation documents and tests the existing brace contract:
  a lone `}` is text, while a lone `{` starts interpolation and must be escaped as `{{`
  when meant literally.
- OUT-3 (B62) — `fmt` preserves an over-indented own-line body comment and
  re-emits it at the block indent instead of dropping it during indentation
  recovery.
- CAT-2 (B70) — verified closed: `marrow test` binds a proposed baseline catalog
  over fresh in-memory stores, so test-first saved writes resolve without a prior
  durable run or evolve apply.
- DUR-2 (B56) — `marrow data recover` does a write-capable repair open for
  stores that report typed `store.recovery_required`, while ordinary read-only
  inspection stays fail-closed on missing or corrupt native stores.
- DUR-6 (B48) — rejected `restore` validates target binding before opening and
  rolls back a newly-created target store file, including dangling symlink targets.
- B-RESTORE-EMPTY — `restore` rejects catalog-only targets instead of treating
  them as empty data stores.
- EVO-4(c)/EVO-5 (B40/B61) — catalog digests are canonical and order-insensitive
  with bounded legacy digest normalization; pure enum-member reorder auto-restamps
  instead of bricking a stamped store; stale `evolve apply` suppresses already-applied
  transforms using historical applied-step evidence without replaying over later data.
- DIAG-4/5 (B41/B63) — runtime output streams live through uncaught faults, and
  `run --trace` / `run --dry-run` flush JSON/JSONL reports on fault paths.
- DIAG-6 (B55) — absent whole-record coalesce yields the fallback while
  present-but-malformed records still fault.
- Check/run consistency (B45/B46/B64/B65) — the checker rejects non-renderable
  print/write/interpolation arguments, rejects unsupported `exists(next(...))` /
  `exists(prev(...))` values, and rejects nested local-resource writes that runtime
  cannot support.
- OUT-1/2 (B67/B74/B51) — `data dump` / `data get` text rendering quotes and
  escapes string values, renders bytes as `0x<hex>`, renders identity references as
  saved paths, and renders enum values as member identities through catalog facts.
- OUT-4 (B49) — `marrow test --format json|jsonl` emits structured per-test
  records and summaries.
- B-STORE-CODEC-1/2 — `marrow-store` now shares one bounds-checked length-prefixed
  reader and one paged scan driver across catalog, metadata, backup, memory, and
  redb traversal paths.

## Remaining

No remaining items from this plan.

## Notes

- Do not edit tracked spec docs ahead of the implementation that realizes a decision; move
  `docs/language/` and contract docs in lockstep with the change.
- `marrow serve`/`marrow lsp` open one store at a time on a single thread; the DUR catch_unwind
  panic-hook swap relies on that. Revisit it before any concurrent store-open is introduced.
