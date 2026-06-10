# Wave 2 — Remaining Policy Fixes (continuation plan)

Status as of 2026-06-10. Wave 1 (engine-resident-catalog) and the highest-severity
Wave 2 behavioral fixes are shipped to `main`. This file is the forward-only backlog
for the rest of the owner-approved policy decisions and the probe findings they map to.

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

## Remaining, by cluster (decided fix in brackets)

### Durability / recovery
- **B56** — SIGKILL mid-write can brick a store with no recovery path. [DUR-2: ship a
  `marrow data recover` verb that does a write-capable repair open and reports survival;
  hangs off the typed `store.recovery_required` from DUR-4.]
- **B48** — a rejected `restore` leaves an orphan `marrow.redb` in a previously-pristine
  `.data`, violating rollback-to-empty. [DUR-6: validate binding before opening the target
  and track/delete the created file on a rolled-back restore.]
- **B59** — restore silently accepts tampered commit metadata (injected `commit_id` skews
  future numbering). [LOW: cover the full commit descriptor by the archive checksum or
  reject a self-inconsistent manifest; the manifest is now checksum-covered (DUR-5), so
  confirm the `commit_id` path is included.]

### Evolution
- **B40** — a pure enum-member reorder bricks a stamped native store (check passes, run
  fences `run.schema_drift`, evolve apply cannot recover). [Reorder is identity-preserving:
  it should re-stamp the durable shape via auto-apply, not brick. Investigate the run-time
  fence vs auto-apply classification for a member-order-only change.]
- **B61** — a stale `evolve apply` re-runs an already-applied transform and corrupts derived
  data. [EVO-5: suppress an already-applied transform keyed on the stamped state match.]
- **EVO-4(c)** — drop member order from the catalog digest (order-insensitive / canonical
  rendering). [Deferred refactor-sequenced store-format follow-up; Wave 1 keeps order via an
  ordinal, which is correct but not order-insensitive.]

### Diagnostics / check-run consistency
- **B41** — all program output is lost when a run ends with an uncaught error/fault.
  [DIAG-4: STREAM program output live; changes the `run_entry_with_host` signature
  marrow-lsp consumes — land the Marrow streaming API first, then adapt the LSP.]
- **B63** — `run --trace`/`--dry-run` with `--format json|jsonl` drops the whole trace/plan
  report on a faulting run (text shows it). [DIAG-5: flush the trace/dry-run report on the
  `Err` arm for format parity.]
- **B55** — a whole-record `^record(key) ?? fallback` faults at runtime on an absent keyed
  record despite a clean check. [DIAG-6: an absent whole-record `??` yields the fallback; a
  present-but-malformed record still faults.]
- **B66** — an unsupported string escape (`\q`) passes check then faults at run with a false
  "runtime does not evaluate escapes" message. [DIAG-2: reject the bad escape at check and
  delete the false message (one-line `syntax.md` edit).]
- **B69** — a nonexistent std op in a real module passes `check`, then faults cryptically with
  no op name or location. [DIAG-1: validate ops at check for the closed pure modules
  (math/text/bytes/assert); host modules stay module-open. Propose-first.]
- **B72** — `Error.code` is a plain string, not `ErrorCode`; invalid codes pass check and run.
  [DIAG-3: enforce `ErrorCode` at both check and run via the shared `is_error_code_text`.]
- **B65** — print/write/interpolation accept non-renderable values caught only at runtime.
  [The checker should reject a non-renderable interpolation/print argument; coordinate with
  B45 below.]
- **B45** — interpolating a date/instant/duration value checks clean then faults
  `run.unsupported` (the checker rejects bytes/enum but not temporal scalars). [Either render
  temporal values in interpolation, or reject them at check like bytes/enum — pick one and
  apply it uniformly with B65.]
- **B46** — `exists(next(...))` / `exists(prev(...))` checks clean but faults at run. [check-run
  consistency: the checker should accept or reject these uniformly with run.]
- **B64** — a nested unkeyed-group field write on a local resource value passes `check` but
  faults at runtime `run.unsupported`. [check-run consistency.]

### Tooling output / formatter / test framework
- **B67** — `data dump`/`data get` text renders string values verbatim, so tabs/newlines break
  the TSV framing and a value can forge a fake record path; string vs bytes is also
  indistinguishable. [OUT-1: mirror the key escaping onto values (quoted/escaped strings,
  `0x<hex>` bytes) — a deliberate contract edit to `data-tools.md`/`cli.md`.]
- **B74** / **B51** — `data dump`/`data get` render an `Id(^store)` reference field as
  undecodable opaque hex, and an enum value as two ragged `$cat_` ids. [OUT-2: render an
  `Id(^store)` as its referent `^authors(1)` and an enum value as one member identity, in the
  OUT-1 value-rendering pass (decode reads catalog stable-id/key facts).]
- **B62** — `fmt` silently deletes an over-indented own-line comment in a function/control-flow
  body. [OUT-3: preserve the over-indented body comment, re-emitted at the block indent. No
  fail-closed stopgap — Marrow is unreleased, go straight to the fix.]
- **B49** — `marrow test --format json|jsonl` is advertised and validated but the test report
  always emits human text. [OUT-4: emit a JSON/JSONL test-result envelope (per-test
  `{name,status,location}` + summary, mirroring `data`/`integrity`).]
- **B70** — `marrow test` fails cryptically on a clean-checking test-first project (tests that
  write saved data error with "catalog identity is missing" until run/evolve apply first).
  [CAT-2: `marrow test` provisions an ephemeral in-memory baseline catalog so a test-first
  project resolves saved-root ids without a prior run.]

### CLI consistency (low)
- **B73** — `marrow lsp` with an extra positional argument exits 1 instead of the documented
  usage error (exit 2).
- **B75** — duplicate `--entry`/`--port` silently take the last value while duplicate
  `--format` is a usage error (exit 2); make duplicate-flag handling uniform.
- **B68** — interpolation accepts a lone `}` as a literal brace while a lone `{` is a hard
  error; document or unify the asymmetry.

## Deferred engineering backlog (from reviews)

In `.build/marrow-hardening/BACKLOG.md`:
- **B-STORE-CODEC-1/2** — three near-duplicate length-prefixed byte cursors and a duplicated
  paged-scan driver in `marrow-store`; extract one shared bounds-checked reader and one page
  driver. Pre-existing; do as a focused store-codec cleanup.
- **B-RESTORE-EMPTY** — `restore` uses `is_empty()` (data+index only) to gate a target; a store
  holding only a baseline catalog passes. Safe today; tighten to reject a target that already
  holds a catalog for a cleaner contract.

## Notes

- Do not edit tracked spec docs ahead of the implementation that realizes a decision; move
  `docs/language/` and contract docs in lockstep with the change.
- `marrow serve`/`marrow lsp` open one store at a time on a single thread; the DUR catch_unwind
  panic-hook swap relies on that. Revisit it before any concurrent store-open is introduced.
