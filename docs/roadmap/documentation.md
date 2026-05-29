# Documentation Worklist

This list tracks documentation gaps to close before the first release. It is a
planning list, not a language design change. Each item should become a concise,
developer-friendly reference page or a small update to an existing page.

## First Pass

- Add `docs/quickstart.md`: create a project, write one resource, run it,
  inspect saved data, and run tests.
- Add `docs/cli.md`: document `marrow check`, `fmt`, `run`, `test`, `backup`,
  `restore`, `data`, `lsp`, and `serve` with command syntax, inputs, outputs,
  exit behavior, and examples.
- Add `docs/project-config.md`: define `marrow.json`, including `sourceRoots`,
  `run.defaultEntry`, `store.backend`, `store.dataDir`, and `tests`.
- Add `docs/data-modeling.md`: explain roots, child layers, identity keys,
  sparse fields, required fields, relationships, history, indexes, and common
  lookup patterns.
- Add `docs/migrations.md`: explain schema changes, `@id`, required-field
  population, index rebuilds, maintenance mode, repair, and restore policy.
- Expand `docs/error-codes.md`: list the stable `parse.*`, `check.*`,
  `schema.*`, `write.*`, `run.*`, `store.*`, `protocol.*`, `config.*`,
  `project.*`, and `io.*` codes that are part of the public surface.

## Tooling References

- Add `docs/data-tools.md` or fold into `docs/cli.md`: define raw inspection,
  stats, dump, integrity checks, and `get` (all implemented and read-only by
  default), with the rule that inspection never creates the store. `diff` and
  `load` remain deferred (they overlap restore's replace/merge/repair modes and
  need typed source-fingerprinting); document them only once implemented.
- Add `docs/serve-protocol.md`: document newline-delimited JSON requests and
  replies, path segment JSON, key JSON, base64 values, error replies, and
  paging limits for `saved_walk`.
- Add `docs/lsp.md`: document the current LSP surface and the planned path from
  parse diagnostics to checked project facts.
- Add `docs/backend-contract.md`: define ordered path/value operations,
  savepoints, presence states, child-key ordering, bounded scans, limits,
  conformance expectations, and native-store responsibilities.

## Language Reference Clarifications

- Expand string and byte literal documentation with escape rules, invalid
  escapes, multiline behavior, interpolation escaping, and byte-literal limits.
- Clarify resource constructors for sparse fields, required fields, unkeyed
  nested groups, keyed layers, and sequence-shaped members.
- Clarify checker rules for definite assignment, sparse-field narrowing,
  resource completeness, `out` and `inout` writeback, return checking, and
  builtin shadowing.
- Clarify transaction and lock guarantees: read-your-writes, nested savepoints,
  host effects, lock scope, missing lock capability, and concurrency limits.
- Expand standard-library edge cases for clock parsing, UTC behavior, text
  length, split, base64, IO replacement behavior, environment lookup, logging,
  and numeric overflow.

## Keep Out Of The First Release Docs

- Do not document a second query language, ORM layer, migration DSL, package
  registry, web framework, users-and-roles system, remote database product, or
  bundled external database adapters.
- Do not add compatibility guidance for Classic M, globals, routines, Postgres,
  or alternate language modes.
- Do not describe backend-specific APIs as application APIs. Application APIs
  are Marrow functions over typed resources and saved data.
