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
| **3 — CLI / LSP boundary** | thin boundary checks | structured-first (parsed JSON/JSONL codes, payloads, exit) + a small reviewed golden set for genuinely human-rendered text |
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
