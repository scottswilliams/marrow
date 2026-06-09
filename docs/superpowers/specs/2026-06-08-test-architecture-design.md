# Marrow Test Architecture (v0.1) — Design

Status: **approved** (brainstorm 2026-06-08). Implemented as wave **R11** in
`docs/roadmap/release-hardening-operating-plan.md`, scheduled **after** the current
hardening waves (R1/R4/R5/R9) so the re-architecture lands on a stable test base and
does not collide with hardening lanes that also touch tests.

## Goal

Re-architect the Marrow test suite to be **scalable, maintainable, and effective**, while
**increasing** meaningful coverage — never reducing it. Kill duplication and sediment,
strengthen oracles, split oversized suites, and close coverage gaps, with heavy emphasis on
the **language↔database seams** and **durable-data edge cases**. No legacy or prototype
behavior may survive in tests.

## Current state (the problems)

~51K test LOC across 101 files / ~490 tests; ~75% already drive the production pipeline
(good bones). Problems:

- **Duplication**: fact-lookup helpers (`member_catalog_id`, `root_place`, catalog-entry
  builders) and fixture schemas (Books/Counter/Enrollment) re-declared 2–5× across crates;
  project-setup boilerplate repeated ~41× in marrow-check alone; no shared fixture corpus.
- **Oversized catch-all suites**: `evolution_apply.rs` 3194, `parse.rs` 2850,
  `discharge_nested.rs` 1738, `eval_lowering_debug_dispatch.rs` 1432, `eval_aggregation.rs`
  1241, `architecture.rs` 1185, `binding_index.rs` 1132, `lsp_cli.rs` 928, …
- **Brittle oracles**: ~100+ prose `.contains()` assertions in CLI tests despite the CLI
  emitting structured JSON/JSONL; 4 source-text `*architecture*.rs` scans assert structure,
  not behavior.
- **Coverage gaps**: multi-store transactions, cross-module saves, evolution failure/unblock
  lifecycle, index-maintenance edges, `analyze_project`/LSP correctness, error-recovery
  cascades, store-backed faults, catalog-commit errors, entry-not-found, and the interaction
  seams (typing × saved-data, presence-proof correctness, transform correctness).

## Decisions (from brainstorm)

1. **Sequenced after hardening.** R11 runs once R1/R4/R5/R9 are integrated.
2. **Shared `.mw` fixture corpus + single-owner helpers.** No shared test-support *crate*
   (avoids Cargo dev-dependency cycles across the syntax→…→cli DAG); fixture *data* has no
   deps, so each crate loads a fixture and runs it through its own production pipeline.
3. **Structured-first oracles + a tiny reviewed golden set** for human-rendered output.
4. **New Tier-2 practical scenarios**, focused on lang-db seams + db edge cases.
5. **Legacy & ADR-alignment audit**; no legacy survives; coverage may not drop.

## Architecture

### Test tiers — every test names its tier, harness layer, fixture, and oracle

| Tier | Scope | Allowed oracle |
|---|---|---|
| 0 — Laws | one component, no pipeline | syntax AST shape / store codec bytes / schema facts directly |
| 1 — Invariants (the bulk) | through the production pipeline | typed: diagnostic codes + payloads, runtime values/effects, store effects, evolution witnesses |
| 2 — Practical scenarios (new) | end-to-end real `.mw` apps: check → run → save → evolve → re-run | observable behavior over the shared corpus |
| 3 — CLI/LSP boundary | thin boundary checks | structured-first (JSON/JSONL codes/payloads/exit) + tiny reviewed golden set for rendered text |
| 4 — Architecture backstops | source-structure absence guards | identifier-aware scans, minimal, paired with positive behavior coverage; prefer a real type boundary where one can express the rule |

The tier dictates the allowed oracle: Tier 1 may never assert on rendered prose; Tier 3
prose lives only behind a reviewed golden.

### Shared `.mw` fixture corpus + consolidated harness

- A versioned `fixtures/` corpus of canonical `.mw` projects — Books, Counter, Enrollment,
  evolution baselines, **plus realistic apps/algorithms for Tier 2** (small ledger, task
  tracker, graph/BFS, etc.). Each crate loads a fixture and runs it through **its own**
  production pipeline.
- **One owner per helper.** Duplicated fact-lookup / catalog-entry builders collapse into
  `cfg(test)`/test-support APIs on the production crate that owns the concept; consumed
  everywhere. Eliminates hand-built replicas.
- **Thin per-crate harness** keeps only crate-specific glue (temp project, store open,
  run/assert wrappers), with consistent naming (resolve `run`/`run_full`/`run_entry` and
  `checker_rejects`/`assert_*`).

### Oracle policy

- Semantic assertions use codes / typed payloads / facts / witnesses / values through the
  production pipeline — never prose substrings.
- Human-rendered output (help text, a few canonical messages) gets a small, explicitly-
  reviewed golden set, regenerated only on intentional change. The ~100+ CLI `.contains()`
  checks convert to parsed JSON/JSONL assertions or move behind a golden.

## Cleanup

### Legacy & ADR-alignment audit (no legacy survives)

Every test is classified **keep / migrate-to-v0.1 / delete**, flagged on:

- **Legacy-protecting** — asserts rejected/prototype behavior, fallback paths, or old
  formats → migrate to the v0.1 contract or delete.
- **Out-of-date / ADR-misaligned** — disagrees with current `docs/language` + accepted ADRs
  (catalog-invisible, native-only surface, etc.) → fix or delete.
- **Low-value / duplicate-invariant** — trivial plumbing (`1+1=2`) or the same invariant
  re-verified across suites → collapse to one owning test.
- **Prose-coupled** — semantic assertion on rendered text → convert to a typed oracle.

Hard rule: **coverage may not drop.** Any invariant a deleted test legitimately guarded must
first be re-expressed as a typed Tier-1 test or folded into a Tier-2 scenario.

### Oversized-suite decomposition

Split the catch-alls by invariant into focused files (soft ceiling ~400–500 lines). Each
split file names its invariant.

## Coverage improvement — Tier 2 lang-db seams & db edge cases (net coverage up)

**Lang-db seams** (language semantics ↔ durable data):

- typed field read-back, and evolution **re-typing** of stored values; type narrowing ×
  saved-field reads
- evolution lifecycle check → preview → apply → run for add/retire/rename/retype/
  index-reshape/key-reshape, with post-evolution reads honoring the new contract
- **catalog identity vs stored values** (stable content-independent ids, not path spelling;
  rename keeps identity; epoch transitions)
- **presence proof ↔ actual storage** (the checker's proof is borne out by what the runtime
  reads)
- the four-way distinction **source spelling / public path / physical store key / schema
  identity** never collapsing
- cross-module saves; multi-store transactions with mid-block failure → **full rollback**

**DB edge cases**: empty / first-run / uncommitted-catalog reads; boundary data (empty
groups, single-key layers, deep nesting, paged collections); partial-write rollback &
idempotent resume; unique-index fail-closed, dropped-index cleanup, orphan entries, index
rebuild after evolution; delete (whole-root / keyed-layer / dangling refs); corrupt catalog
→ `store.corruption`; backup/restore round-trip + restore-into-non-empty; **native (redb) vs
mem backend parity** across the above.

These directly fill the audit's gaps, so any deletion is offset by stronger typed coverage.

## Delivery (wave R11)

File-disjoint lanes through the existing deep-lanes harness, each build → no-skim review
(soundness + idiom) → repair → gate:

1. **Foundation lane** — build the `fixtures/` corpus + consolidated single-owner helpers +
   thin per-crate harnesses (no behavior change; pure enablement).
2. **Per-crate migration lanes** (parallel, file-disjoint) — retier tests, run the
   legacy/ADR audit, convert oracles, split oversized suites.
3. **Tier-2 scenarios lane** — the lang-db-seam + db-edge-case suite on the corpus.
4. **Coverage-closeout lane** — fill remaining typed gaps; CLI structured-first + golden set.

Durable reference: **`docs/testing-architecture.md`** records the tiers, oracle policy,
fixture corpus, and the rule that every test names tier/harness/fixture/oracle.

## Non-goals / constraints

- No coverage reduction; deletions are offset first.
- No new runtime or test dependencies without an explicit architecture decision; no shared
  test-support crate.
- Native and mem backends judged at parity.
- Runs after the hardening waves; no concurrency with lanes that touch the same tests.
