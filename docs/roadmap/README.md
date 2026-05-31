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

No accepted implementation issues are currently open.

## Non-Goals

Marrow stays a local language/database kernel. It does not aim to become these:

- a second storage query language;
- an ORM layer or an automatic migration engine;
- a migration DSL before ordinary functions in maintenance mode prove
  insufficient;
- a hidden migration ledger inside the database kernel;
- unchecked dynamic `any` (`unknown` marks dynamic boundaries);
- a built-in users, roles, and permissions system;
