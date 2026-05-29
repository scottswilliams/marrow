# Marrow Roadmap

The Marrow language and database kernel is implemented: the `.mw` parser,
formatter, checker, and runtime; resources as typed local and saved trees;
memory and native (redb) storage behind one backend contract; managed writes
with generated indexes; transactions and savepoints; and the `marrow` CLI,
inspection tools, language server, and data server. The language reference in
[`../language/`](../language/) and the runtime design in
[`../implementation.md`](../implementation.md) describe what exists today.

This page records what Marrow deliberately leaves out, so the boundary stays
clear as the product grows.

## Deferrals

These are coherent future surfaces, not part of the current build:

- `marrow data diff` and `marrow data load` overlap restore's
  replace/merge/repair modes and need typed source-fingerprinting. They will
  route through the maintenance capability when implemented, and will not loosen
  the read-only guarantee of the `marrow data` inspection group.
- Replace, merge, and repair restores (the non-empty `marrow restore` cases)
  are deferred. `marrow restore` writes into an empty target only; the
  non-empty cases will be explicit maintenance actions routed through the
  maintenance capability, not a relaxation of the empty-target guard.
- Custom identity allocation policies wait until single-`int` allocation is
  fully exercised in practice.

## Non-Goals

Marrow stays a local language/database kernel. It does not aim to become these:

- bundled external database adapters;
- alternate language modes, or compatibility paths for Classic M, globals,
  routines, or Postgres;
- a second storage query language;
- an ORM layer or an automatic migration engine;
- a migration DSL before ordinary functions in maintenance mode prove
  insufficient;
- a hidden migration ledger inside the database kernel;
- unchecked dynamic `any` (`unknown` marks dynamic boundaries);
- an HTTP framework contract;
- a built-in users, roles, and permissions system;
- an external package registry.

## Verification

Documentation-only work uses link scans, stale-term scans, and
`git diff --check`. Parser, checker, runtime, CLI, LSP, and backend work starts
with focused tests for the changed surface, then grows to `cargo fmt --all`,
`cargo test --workspace`, and the conformance suites for integration batches.
