# Marrow Roadmap

The Marrow language and database kernel is implemented: the `.mw` parser,
formatter, checker, and runtime; resources as typed local and saved trees;
memory and native (redb) storage behind one backend contract; managed writes
with generated indexes; transactions and savepoints; and the `marrow` CLI,
inspection tools, language server, and data server. The language reference in
[`../language/`](../language/) and the runtime design in
[`../implementation.md`](../implementation.md) describe what exists today.

Surfaces that are designed and normative but not yet implemented live under
[`../future/`](../future/), the future counterpart of this reference. This page
records accepted implementation issues and the non-goals that bound the
product, so the boundary stays clear as it grows.

## Implementation Roadmap

The active implementation plan and forward tracker is
[`prototype-to-v1-execution-plan.md`](prototype-to-v1-execution-plan.md). It
maps the accepted Marrow ADR packet to file-disjoint lanes, review gates,
deletion targets, and verification commands for the v0.1 rewrite.

The per-orchestrator tracking plans live under [`lanes/`](lanes/). Start new
implementation orchestrators from those files so worktree ownership, target
directories, dependencies, deletion ledgers, and review prompts stay consistent.

Closed implementation records:

- [#1 Implement element-oriented collection loop semantics](https://github.com/scottswilliams/marrow/issues/1)
  is closed; the accepted element-oriented loop rule now lives in the language
  reference.

## Non-Goals

Marrow stays a local language/database kernel. It does not aim to become these:

- a second storage query language;
- an ORM layer or a SQL-style migration subsystem;
- a separate migration DSL as the primary data-evolution model;
- a hidden database-owned migration ledger outside Marrow's compiler/catalog
  model;
- unchecked dynamic `any` (`unknown` marks dynamic boundaries);
- a built-in users, roles, and permissions system;
