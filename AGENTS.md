# Agent Instructions

This repository is Marrow.

Marrow is a lightweight, typed `.mw` language with built-in saved data. Data is
scalars or trees. A resource is a typed tree; the same shape can be local or
saved, and `^` marks saved data. Marrow is its own language and database model,
not a layer on another system. Durable data stays under Marrow's language and
tooling contract regardless of which storage engine holds the bytes.

`docs/language/` is the canonical source for Marrow language behavior. Parser,
checker, runtime, CLI, LSP, examples, tests, and other docs converge on that
directory. When implementation and documentation disagree, treat the
disagreement as implementation work, not as a competing design.

Implementation and tooling references live in concise `docs/` pages such as
the backend, server, and roadmap references. Keep them simple, current, and
organized like a real language/database reference. The code itself should be
self-documenting where possible.

Marrow is unreleased. Do not preserve stale names, old design formats,
obsolete examples, or transition shims for their own sake. When the design
changes, clean the repository as if the new design had been here from the
beginning: simple, direct, and inspectable.

Green tests or compile success are not reasons to keep legacy prototype paths.
If a test or fixture depends on outdated behavior, update or delete it so it
asserts the v0.1 contract. Runtime or CLI callers must migrate unless the
lane's prototype-removal ledger names a live production bridge with caller,
isolation boundary, absence test, and deletion lane, or the surface is
explicitly debug/admin-only and excluded from production semantics. Do not keep
fallback branches, mode flags, compatibility shims, test-only production entry
points, or duplicate semantic models just to preserve old behavior.

Avoid agentic slop and documentation sediment at all costs, including in code.

## Working Rules

- Read `docs/language/` before changing `.mw` syntax, typing, resources,
  builtins, saved data behavior, or user-facing terminology.
- Keep documentation in the voice of a real language reference: precise,
  current, and useful to everyday developers.
- Prefer deleting stale files over moving them around. If useful content
  survives, fold it into the smallest durable reference page.
- Avoid unrelated reverts. The worktree may contain user or agent changes;
  understand them and work with them.
- Keep edits scoped and verify what changed. Do not claim completion without
  fresh command output.

## Engineering Rules

1. Think before coding. State assumptions, surface tradeoffs, push back when a
   simpler approach exists, and ask when guessing would risk the work.
2. Simplicity first. Write the minimum code that solves the problem. Avoid
   speculative features, single-use abstractions, and unrequested
   configurability.
3. Surgical changes. Touch only what the request requires. Match existing
   style. Every changed line should trace directly to the user's request.
4. Goal-driven execution. Turn tasks into verifiable goals. For behavior
   changes, write or identify the failing check first, then make it pass.

## Worktrees

Use an isolated worktree for multi-file changes, Rust changes, or cleanup
batches. Keep lane worktrees under `/Users/scottwilliams/Dev` next to the main
checkout, using names such as `/Users/scottwilliams/Dev/marrow-<lane>`.

Keep harness files, throwaway worktrees, cargo targets, trial artifacts,
patches, reviews, logs, and leases outside the repository. The tracked repo
contains source, tests, and durable reference docs only.

## Verification

Use focused checks before broad ones:

1. no-compile checks: `git diff --check`, stale-term scans, link scans,
   markdown checks, and formatting of touched docs;
2. focused Rust checks: the smallest package, library, or test target that
   proves the change;
3. workspace checks: `cargo build --workspace` and `cargo test --workspace`
   for broad rename, runtime, or release-surface changes.

Do not run broad Cargo gates in parallel against the same target directory. In
a lane, spell `CARGO_TARGET_DIR` explicitly in every Cargo command, using an
external target path:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/<lane> \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-<lane>/Cargo.toml ...
```

Use `/Users/scottwilliams/Dev/.build/marrow-targets/integration` for broad
integration gates, one at a time.

## Review And Integration

Agents do not merge feature branches directly into `main` without review. Use
short-lived branches, small commits, focused verification, and a read-only
high-reasoning review before integration.

Review prompt:

```text
Review this branch against main. Findings first. Focus on correctness,
simplicity, minimality, and whether every changed line belongs. Treat
docs/language/ as the source of truth for language behavior.
```

Integrate only from the live main checkout at `/Users/scottwilliams/Dev/marrow`.
Prefer `git cherry-pick -x <reviewed-sha>` over merging a whole branch. If a
conflict is not an obvious mechanical rename/import conflict, abort and send the
branch back to the lane.

Before pushing `main`, run the verification ladder through the workspace checks
using `/Users/scottwilliams/Dev/.build/marrow-targets/integration`, then ask for
a final read-only review of the assembled diff:

```text
Review the integration diff before main is pushed. Findings first. Focus on
correctness, simplicity, minimality, and whether the cherry-picked commits
belong together.
```

## Repository Shape

- `docs/language/` is the language reference.
- `docs/implementation.md` is the implementation and backend reference.
- `docs/roadmap/` is a status note: the implemented kernel, plus the deferrals
  and non-goals that bound it.
- Other durable language, database, implementation, backend, tooling, and
  roadmap docs belong under `docs/`, not scattered elsewhere.
- Public examples and demos exist only when they match `docs/language/` and
  the implementation. Otherwise remove them and keep coverage in tests or
  fixtures.

## Coding Expectations

- Prefer simple Rust and narrow abstractions.
- Prefer typed IDs and small enums over strings or booleans when values carry
  semantic identity or state.
- Avoid crate-root glob preludes and production `use super::*`; import the names
  a module uses.
- Do not duplicate semantic classifiers across parser, checker, runtime, tools,
  or tests. Move the ownership boundary instead.
- Delete obsolete prototype paths instead of wrapping them in compatibility
  shims.
- Keep code concise and self-documenting. Prioritize readability and
  maintainability.
- Write comments as a human engineer would: explain *why*, in plain prose. Do not
  cite docs by filename or line, reference tickets, reviews, roadmap steps, or
  wave/slice numbers, narrate edits ("previously", "now changed to"), or restate
  what the code already says. State the rationale directly and trust the reader to
  find the rest in `docs/`.
- Follow the 80/20 rule: avoid large changes without proportionate impact.
- Add tests near the behavior being changed.
- Keep storage behavior behind the backend contract; a backend is a store target,
  not the owner of Marrow semantics.
- Keep saved data inspectable through Marrow tools.
- Keep the repository Apache-2.0 only.
- Keep native `.mw` as the only default language surface.
- Do not add legacy language modes or code paths, transition layers, or
  alternate default product surfaces.
- Do not bundle external database adapters in the first release.
- Do not introduce `unsafe` Rust.
- Look for opportunities to dogfood with `.mw` where it makes sense.

Before committing substantial code changes, spawn a high-reasoning read-only
review of the staged and unstaged changes:

```text
Review the staged and unstaged changes. Findings first with file and line
references. Focus on minimal, simple, self-documenting code, and whether every
changed line belongs. Would a senior programming language developer sign off on
this code? Is this the best, simplest solution?
```
