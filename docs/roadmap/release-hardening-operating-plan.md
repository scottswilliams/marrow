# Marrow Release Hardening Operating Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development for substantial lanes. Use superpowers:executing-plans only for narrow, file-local cleanup. Steps use checkbox (`- [ ]`) syntax for lane-local tracking.

**Goal:** Finish the remaining Marrow v0.1 roadmap as a release-hardening program that deletes prototype residue, strengthens durable language/database foundations, and leaves the Rust codebase easy to review.

**Architecture:** The rust-hardening file audit remains the per-file evidence ledger. This plan defines the operating model, lane order, delegation rules, review gates, and code-quality bar for the remaining work. Every implementation lane must make the v0.1 architecture more direct: one semantic owner per concept, typed facts over strings, explicit storage boundaries, and no legacy compatibility path preserved for green tests.

**Tech Stack:** Rust workspace, Cargo, isolated git worktrees, lane-specific `CARGO_TARGET_DIR`, Marrow language docs, accepted ADRs, checker/runtime/store/CLI integration tests, read-only soundness and idiom/spec review agents.

---

## Operating Rules

- [ ] Treat Marrow as release-hardening work, not feature expansion. New user-facing semantics require an explicit language/database decision before implementation.
- [ ] Start every substantial lane from a fresh worktree and a lane-specific build-output directory outside the repository.
- [ ] Keep lanes file-disjoint when running concurrently. Sequence lanes that touch shared checker tests, runtime fixtures, CLI tests, docs/language semantics, or store contracts.
- [ ] Build with one implementation agent, then review with at least two read-only agents: soundness and idiom/spec.
- [ ] Fix every review finding before integration. A real but out-of-scope issue becomes a concrete backlog item with owner area and blocking reason.
- [ ] Re-run review after fixes for soundness-critical areas: identity, catalog, schema evolution, store keys, write paths, backup/restore, integrity, and activation.
- [ ] Integrate only after live-main recheck, rebase or cherry-pick onto current main, focused gates, full gates, review pass, and clean status.
- [ ] Do not preserve compatibility glue, fallback branches, mode flags, duplicate semantic models, or test-only production entrypoints to keep old behavior passing.
- [ ] Do not keep active-lane sediment in tracked docs: no temporary worktree paths, target paths, stale branch names, or completed-history narrative in durable docs.

## Code-Quality Bar

Every lane must enforce these rules in its owned area before review:

- [ ] Delete prototype code instead of wrapping it. If the v0.1 contract rejects a surface, tests and docs must reject it too.
- [ ] Keep one semantic owner per concept: saved paths, catalog identity, stored values, evolution verdicts, diagnostics, runtime effects, store keys, backup payloads, and tool facts.
- [ ] Prefer typed IDs, typed facts, small enums, and explicit state over raw strings, booleans, source spelling, public path text, or diagnostic prose.
- [ ] Keep public APIs narrow. Raw backend bytes, raw paths, debug/admin inspection, and archive internals must not become production semantics.
- [ ] Split broad dispatchers before claiming done. Long match-heavy functions need focused helpers or modules when the branch structure encodes multiple invariants.
- [ ] Remove duplicate classifiers and helper families. A helper that restates parser/checker/runtime knowledge in another layer is a blocking smell.
- [ ] Remove low-value comments. Keep comments only for durable rationale, representation invariants, cost, recovery behavior, or soundness.
- [ ] Replace prose semantic assertions with typed assertions when the production pipeline exposes a fact, diagnostic code, witness, store effect, or runtime value.
- [ ] Keep source-text architecture scans as absence backstops only. They must be identifier-aware and paired with positive behavior coverage.
- [ ] Keep tests production-pipeline-first. Avoid hand-built replicas of compiler/runtime semantics in tests.

## Evidence Packet

A lane may report complete only when it returns all of this:

- [ ] Base commit, final head commit, and live-main head used for integration.
- [ ] Exact changed-file list.
- [ ] The failing check or gap evidence that justified behavior changes.
- [ ] Focused gate output with explicit manifest path and explicit lane target dir.
- [ ] Full gate output: workspace tests, formatter, clippy with `-D warnings`, and unsafe scan.
- [ ] Soundness review verdict, findings, fixes, and re-review verdict.
- [ ] Idiom/spec review verdict, findings, fixes, and re-review verdict.
- [ ] Absence and sibling scans across the entire owned area.
- [ ] Confirmation that docs, tests, fixtures, and product surfaces match the v0.1 contract.
- [ ] Clean lane status after integration and retired worktree status or explicit owner handoff.

## Remaining Lane Order

### R0: Coordination And Tracker Reconciliation

Purpose: make the operating surface reliable before assigning more agents.

- [ ] Refresh the rust-hardening tracker against current `main`.
- [ ] Remove stale inventory entries for deleted product surfaces.
- [ ] Resolve or hand off active worktree and dirty-state ownership.
- [ ] Confirm each pending lane has current file ownership and no overlap with active hardening work.
- [ ] Gate: clean status or explicitly owned dirty state, current tracker head, and no lane prompt pointing at deleted files.

Run R0 before new implementation lanes.

### R1: CLI, Tooling, Serve, And Data Surface Closure

Purpose: make every product-facing tool surface match the v0.1 story after prototype feature deletion.

Owns:

- `/Users/scottwilliams/Dev/marrow/crates/marrow/src/main.rs`
- `/Users/scottwilliams/Dev/marrow/crates/marrow/src/cmd_data.rs`
- `/Users/scottwilliams/Dev/marrow/crates/marrow/src/cmd_data/`
- `/Users/scottwilliams/Dev/marrow/crates/marrow/tests/`
- `/Users/scottwilliams/Dev/marrow/docs/cli.md`
- `/Users/scottwilliams/Dev/marrow/docs/data-tools.md`
- `/Users/scottwilliams/Dev/marrow/docs/serve-protocol.md`
- `/Users/scottwilliams/Dev/marrow/docs/tooling-surfaces.md`

Must prove:

- [ ] No `marrow explain` command, alias, docs surface, or debug equivalent remains.
- [ ] `marrow data` and `marrow serve` debug/admin behavior is typed, bounded, and not a production app API.
- [ ] Help text, docs, tests, and architecture scans agree.
- [ ] CLI assertions use typed/stable facts where available and prose assertions only for rendering boundaries.
- [ ] No raw saved path, backend byte, or query-plan concept leaks into production tooling.

### R2: Store Boundary And Value Encoding

Purpose: make the storage layer a durable typed substrate, not a raw prototype backend.

Owns:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-store/src/`
- `/Users/scottwilliams/Dev/marrow/crates/marrow-store/tests/`

Must prove:

- [ ] Stable schema identity, public path text, physical store keys, and backend bytes are separate concepts.
- [ ] Mem and native backends share one conformance contract.
- [ ] Raw constructors are private to archive/debug/admin boundaries and cannot be selected by production callers.
- [ ] Tree/store code is reviewable; split oversized functions only where it removes real complexity.
- [ ] Tests cover ordering, cursors, snapshots, value encoding, integrity, and backend parity without source-text semantic replicas.

### R3: Backup, Restore, Integrity, And Repair

Purpose: verify durable data recovery is typed Marrow recovery, not engine-byte copying.

Depends on R2.

Owns:

- Backup and restore modules in `/Users/scottwilliams/Dev/marrow/crates/marrow-store/src/`
- Data integrity command surfaces owned jointly with R1 only after R1 completes or grants file ownership.
- `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md`
- `/Users/scottwilliams/Dev/marrow/docs/data-tools.md`

Must prove:

- [ ] Restore targets are empty or explicitly safe by policy.
- [ ] Restore validates typed backup payloads against compiler/catalog/data-integrity contracts.
- [ ] Orphaning and repair guidance is compiler/data-integrity driven.
- [ ] Backup format does not expose raw engine records as stable user semantics.
- [ ] Recovery tests cover invalid payloads, stale catalog identity, interrupted restore, and repair guidance.

### R4: Checker And Runtime Evolution Soundness

Purpose: make evolution trustworthy because compiler and database are one system.

Owns in sequenced batches:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/`
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/tests/evolution_discharge.rs`
- Runtime evolution code and tests under `/Users/scottwilliams/Dev/marrow/crates/marrow-run/`
- Evolution docs only when semantics change.

Must prove:

- [ ] Evolution obligations are represented as typed facts and witnesses.
- [ ] Runtime activation consumes checked facts rather than reclassifying language structure.
- [ ] Defaults, transforms, field retire, index rebuild, dangling references, and catalog epochs are covered through production fixtures.
- [ ] No migration-script, SQL patch, global lock, or old/new duplicate semantic model remains.
- [ ] Oversized tests are split by invariant when reviewability is impaired.

### R5: Runtime Core, Reads, Writes, And Transactions

Purpose: harden runtime execution and write behavior after prototype feature removal.

Owns:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/`
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/tests/`

Must prove:

- [ ] Runtime uses checked facts for identity, effects, value shape, and store access.
- [ ] Writes are explicit checked effects; no saved-path inout, hidden destructive root assignment, or merge-like compatibility path survives.
- [ ] Trace, dry-run, and maintenance modes do not change production semantics.
- [ ] Large runtime tests are split or converted to focused fixtures where reviewability is poor.
- [ ] Runtime APIs provide the facts downstream tools need without exposing raw prototype internals.

### R6: Checker Core Simplification

Purpose: make checker code readable enough to trust.

Owns:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/checks.rs`
- Checker core modules under `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/`
- Checker project tests only when shared ownership is granted.

Must prove:

- [ ] Broad dispatcher functions are split into focused invariant checks.
- [ ] Diagnostic production is typed by code and span; prose is render-only.
- [ ] Saved path, builtin, identity, presence, and effect classification each have one owner.
- [ ] Tests assert diagnostics/facts/effects through the production pipeline.
- [ ] Comments explain invariants, not branches, history, or migration status.

### R7: Syntax, Parser, And Formatter Simplification

Purpose: keep syntax code syntax-only and reviewable.

Owns:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-syntax/src/`
- `/Users/scottwilliams/Dev/marrow/crates/marrow-syntax/tests/`

Must prove:

- [ ] Parser code does not encode checker/runtime semantics.
- [ ] Large declaration parsing code is split where it improves ownership and testability.
- [ ] Parse tests assert structure and formatting behavior without duplicating semantic rules.
- [ ] Rejected prototype constructs remain parser-accepted only when the checker owns rejection by accepted design.
- [ ] Formatter output preserves syntax faithfully without becoming a second semantic model.

### R8: Schema, Catalog, And Project Model Closure

Purpose: ensure durable schema identity and project discovery are production foundations.

Owns:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-schema/src/`
- `/Users/scottwilliams/Dev/marrow/crates/marrow-schema/tests/`
- Project discovery/config files in checker and CLI crates.

Must prove:

- [ ] Catalog IDs are typed, stable, and not derived from public path spelling.
- [ ] Project discovery does not hide glob-prelude or implicit prototype behavior.
- [ ] Schema facts are consumed by checker/runtime/store without duplicate classifiers.
- [ ] Tests cover typed schema facts rather than prose or raw path strings.

### R9: Documentation Final Coherence

Purpose: make docs a durable source of truth rather than a project-memory dump.

Owns:

- `/Users/scottwilliams/Dev/marrow/docs/`
- Top-level project docs only when ownership is explicit.

Must prove:

- [ ] `docs/language/` presents current v0.1 semantics only.
- [ ] Future features are separated from current behavior and marked as deferred or proposed.
- [ ] Docs do not preserve completed lane history, temporary execution details, or old prototype examples.
- [ ] CLI, data, serve, backup, restore, evolution, and tooling docs agree with code and tests.
- [ ] Duplicate headings, stale terms, and unsupported feature surfaces are removed.

### R10: marrow-lsp Handoff Boundary

Purpose: keep Marrow and marrow-lsp aligned without patching LSP around missing Marrow facts.

This is coordination, not Marrow implementation.

- [ ] Hand off current Marrow facts and deleted prototype surfaces to the marrow-lsp lead.
- [ ] Confirm any dirty marrow-lsp roadmap or baseline docs are integrated or explicitly dropped by the LSP lead.
- [ ] Record every LSP blocker as either an existing canonical Marrow API, a required Marrow fact, or LSP prototype code to delete.
- [ ] Do not add Marrow compatibility APIs solely for marrow-lsp.

### R11: Test Architecture Re-Build

Purpose: re-architect the whole test suite to be scalable, maintainable, and effective while
INCREASING meaningful coverage. Full design in
`docs/superpowers/specs/2026-06-08-test-architecture-design.md`.

Runs AFTER R1/R4/R5/R9 integrate, so it lands on a stable test base and does not collide with
hardening lanes that also edit tests.

Owns: all `crates/*/tests/`, in-crate `#[cfg(test)]` test-support, a new versioned `fixtures/`
`.mw` corpus, and a new `docs/testing-architecture.md`.

Must prove:

- [ ] Every test names its tier (0 Laws / 1 Invariants / 2 Practical scenarios / 3 CLI-LSP
  boundary / 4 Architecture backstop), harness layer, fixture, and oracle.
- [ ] Tier 1 asserts typed codes/payloads/facts/witnesses/values through the production
  pipeline; no prose-substring semantic assertions remain.
- [ ] Tier 3 is structured-first (JSON/JSONL codes/payloads/exit); rendered prose lives only
  behind a small explicitly-reviewed golden set.
- [ ] Shared `.mw` fixture corpus exists; duplicated fact-lookup/catalog-builder helpers have
  one owner; Books/Counter/Enrollment are declared once, not 2-5x.
- [ ] Tier 2 practical scenarios exist with heavy focus on language-database seams (typed
  read-back, evolution re-typing, catalog identity vs stored values, presence-proof vs
  storage, source/path/key/identity separation, cross-module saves, multi-store transaction
  rollback) and durable-data edge cases (empty/first-run, boundary/paged data, partial-write
  rollback, index fail-closed/rebuild/orphans, delete semantics, corrupt-catalog, backup
  restore round-trip, native vs mem parity).
- [ ] Legacy/ADR-alignment audit complete: no test protects rejected/prototype behavior, is
  out of date, or is misaligned with docs/language + accepted ADRs; each was kept, migrated to
  the v0.1 contract, or deleted.
- [ ] Coverage did NOT drop: every invariant a deleted test guarded is re-expressed as a typed
  Tier-1 test or a Tier-2 scenario before deletion.
- [ ] Oversized catch-all suites split by invariant under a soft ~400-500 line ceiling.
- [ ] `docs/testing-architecture.md` records the tiers, oracle policy, fixture corpus, and the
  naming rule as the durable contract.

Lanes (file-disjoint, through the deep-lanes harness): (1) Foundation - corpus + single-owner
helpers + thin per-crate harness, no behavior change; (2) Per-crate migration - retier, audit
legacy/ADR, convert oracles, split oversized; (3) Tier-2 scenarios - lang-db seams + db edge
cases; (4) Coverage closeout - fill typed gaps, CLI structured-first + golden set.

## Parallelization Plan

- [ ] Run R0 alone.
- [ ] After R0, R1 and R2 may run in parallel only if R1 avoids store files and R2 avoids CLI/docs files.
- [ ] R3 starts after R2. It must coordinate with R1 before touching data command tests or data-tool docs.
- [ ] R4 and R5 may run in parallel only after their shared runtime fixtures are split or one lane owns them.
- [ ] R6 sequences with R4 when checker evolution tests or checker core facts overlap.
- [ ] R7 can run in parallel with store/runtime lanes when no language semantic changes are proposed.
- [ ] R8 sequences with R6 when catalog/schema facts are touched by checker code.
- [ ] R9 runs late, after semantic and tooling surfaces settle.
- [ ] R10 can run as read-only coordination throughout, but Marrow API changes still require a Marrow lane.
- [ ] R11 runs last, after R1/R4/R5/R9 integrate; its lanes are file-disjoint but sequence the Foundation lane before the per-crate migration lanes.

## Lane Starter Prompt Template

Use this shape for each implementation lane:

```text
You are <lane name>. Marrow is in release-hardening mode, not feature-expansion mode.

Start by reading AGENTS.md, accepted ADRs relevant to your area, docs/language, this operating plan, and the rust-hardening file audit. Inspect current git/worktree state before trusting this prompt.

Work in an isolated worktree and use an explicit lane-specific CARGO_TARGET_DIR in every cargo command. Do not edit unrelated files. Do not create compatibility glue, fallback branches, duplicate semantic models, or test-only production entrypoints.

Your goal is comprehensive cleanup of your owned area: delete prototype residue, remove dead or duplicated code paths, split oversized Rust where it improves reviewability, strip low-value comments, replace prose semantic assertions with typed facts where practical, and prove one semantic owner per concept.

Use TDD for behavior changes. Identify the failing check or coverage gap first. Then implement the smallest production-pipeline change that makes the v0.1 contract true.

Before claiming done, run focused gates, full gates, soundness review, idiom/spec review, fix every finding, rerun review, perform absence/sibling scans, update the hardening tracker, integrate onto live main, and return the full evidence packet.
```

## Final Release Gate

Before v0.1 release readiness is claimed:

- [ ] Every lane status is complete or explicitly blocked by a user decision.
- [ ] No active lane docs contain temporary execution details.
- [ ] `cargo test --workspace` passes with explicit build-output isolation.
- [ ] `cargo fmt --all --check` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] Unsafe Rust scan has no Rust hits.
- [ ] Prototype-surface absence scans pass for rejected language, runtime, store, CLI, and docs surfaces.
- [ ] Public API scan has no raw/fallback/legacy/prototype/helper surface without a production owner and boundary.
- [ ] Oversized hotspot review confirms each large file is either split or intentionally cohesive with reviewer signoff.
- [ ] marrow-lsp blockers are owned by the LSP lead and do not require Marrow prototype compatibility.
- [ ] R11 complete: tests are tiered with typed oracles, the shared fixture corpus and single-owner helpers exist, the legacy/ADR audit left no legacy-protecting or out-of-date tests, coverage did not drop, and oversized suites are split.
