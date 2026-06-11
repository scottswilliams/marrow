# Testing Architecture

How Marrow's tests are organized, and the rules for adding new ones. Tests exercise the
production pipeline (parser, checker, runtime, store, CLI), not hand-built replicas of it.

## Tiers

Every test belongs to one tier, and the tier dictates the **allowed oracle** — what a test
is permitted to assert on.

| Tier | Scope | Allowed oracle |
|---|---|---|
| **0 — Laws** | one component, no pipeline | syntax/AST shape, store codec bytes, schema facts asserted directly |
| **1 — Invariants** | through the production pipeline (the bulk) | typed: diagnostic *codes* + payloads, runtime *values*/effects, store effects, evolution witnesses |
| **2 — Scenarios** | end-to-end realistic `.mw`: check → run → save → (evolve) → re-run | observable behavior over the shared fixtures |
| **3 — CLI / editor-adapter boundary** | thin boundary checks | structured-first (parsed JSON/JSONL codes, payloads, exit) + a small reviewed golden set for genuinely human-rendered text |
| **4 — Architecture backstops** | source-structure absence guards | identifier-aware scans, kept minimal, always paired with positive behavior coverage; prefer a real type boundary where one can express the rule |

The tier is not a label for its own sake — it is the contract for the oracle. A Tier-1 test
may never assert on a rendered diagnostic message; it asserts the typed code/fact the message
renders from. A Tier-3 test asserts parsed structured output, with prose reserved for the
render contract behind an explicit golden.

## Oracle policy

- Semantic assertions use codes / typed payloads / facts / witnesses / values — never a
  substring match on rendered prose.
- Human-rendered output (help text, a few canonical messages) is pinned by a small,
  explicitly-marked golden assertion, regenerated only on an intentional change.
- The parser and store-codec layers (Tier 0) may assert AST shape and bytes directly; that is
  their contract, not a replica of higher-level semantics.

## Fixtures

Canonical `.mw` projects live under the repo-root `fixtures/` corpus and are loaded by each
crate's thin test support, then run through that crate's own production pipeline (fixture data
has no dependencies, so there is no shared test-support crate and no Cargo cycle). Do not
re-declare the same project as an inline string in multiple crates; add it to the corpus once.

## Rules for adding a test

- **Name the suite by its invariant.** A test file is a focused set of related invariants,
  not a catch-all. Keep files under a soft ~400–500 line ceiling; split by invariant when a
  suite outgrows it (every test moves intact — coverage never drops).
- **One owner per helper.** Shared fact-lookup / fixture-builder helpers live in one place
  (the crate's test support, or the production crate that owns the concept behind a test
  support boundary), never copied across files or crates.
- **Coverage never drops.** When a suite is split or an oracle is strengthened, every original
  invariant is still asserted — by the same test moved intact, or re-expressed as a typed
  Tier-1 test or a Tier-2 scenario before the old one is removed.
- **No legacy.** A test must assert the v0.1 contract. A test that depends on rejected,
  prototype, or removed behavior is migrated to the contract or deleted, not kept green.
- **Comments are durable rationale only.** No narration, history, or restating the assertion.

## Test quality rubric

Every test is judged against the points below. A reviewer flags any a test fails;
a test that fails several is reworked or removed before it is merged.

1. **Behavioral, not existential.** It asserts what the code *does*, not that a symbol,
   field, or module simply exists or has a given structural shape.
2. **Refactor-resistant.** It pins the observable contract — typed codes, payloads, facts,
   witnesses, values, store effects — not the implementation's internal shape, and never a
   substring of rendered prose.
3. **Protects an important contract.** It guards behavior a user or a sibling component
   depends on, not trivia, a tautology, or a restatement of the line under it.
4. **Mutation-resistant.** It would fail if the important behavior broke. It exercises the
   fail-closed path or the edge case, not only the happy path that passes by construction.
5. **Production pipeline.** It runs through the real parser, checker, runtime, store, or CLI,
   not a hand-built replica that can drift from production.
6. **Typed oracle at the right tier.** Its assertions match its tier's allowed oracle —
   no rendered-message check standing in for a typed Tier-1 fact.
7. **Focused and diagnosable.** It pins one invariant, so a failure names the broken
   contract rather than a tangle of unrelated behavior.
8. **Uniquely owned.** No other test already asserts the same invariant; duplicate-invariant
   tests are consolidated, not multiplied.
9. **Deterministic and isolated.** It does not depend on ordering, timing, shared mutable
   state, or another test having run; it sets up and tears down its own world.
10. **Clear intent.** Its name and body state the invariant under test, so a reader sees what
    breaking it would mean.
11. **Asserts the critical DB contracts.** Where it covers durable data, it asserts the
    contract that matters — atomic write-and-rollback, index consistency and rebuild, unique
    fail-closed, identity stability, store-corruption fail-closed, backup/restore integrity,
    and evolution-discharge fail-closed — not an incidental side effect of exercising them.
